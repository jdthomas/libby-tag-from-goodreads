use std::path::PathBuf;

use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use reqwest::header;
use scraper::Html;
use scraper::Selector;
use serde::Deserialize;
use tokio::time::Duration;
use tracing::debug;
use tracing::info;

const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:137.0) Gecko/20100101 Firefox/137.0";
const GOODREADS_BASE: &str = "https://www.goodreads.com";

#[derive(Debug, Deserialize)]
pub struct GoodreadsConfig {
    pub user_id: String,
    pub cookies: String,
}

pub struct GoodreadsExporter {
    client: reqwest::Client,
    config: GoodreadsConfig,
}

impl GoodreadsExporter {
    pub async fn new(config_path: PathBuf) -> Result<Self> {
        let config_data = tokio::fs::read_to_string(&config_path)
            .await
            .with_context(|| format!("reading config from {}", config_path.display()))?;
        let config: GoodreadsConfig =
            serde_json::from_str(&config_data).context("parsing goodreads config JSON")?;

        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::COOKIE,
            header::HeaderValue::from_str(&config.cookies)
                .context("invalid cookie header value")?,
        );
        headers.insert(
            header::USER_AGENT,
            header::HeaderValue::from_static(USER_AGENT),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("building reqwest client")?;

        Ok(Self { client, config })
    }

    async fn scrape_csrf_token(&self) -> Result<String> {
        let url = format!("{}/review/import", GOODREADS_BASE);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("fetching import page for CSRF token")?;

        let final_url = resp.url().to_string();
        debug!("import page final URL: {}", final_url);

        if !resp.status().is_success() {
            bail!(
                "failed to fetch import page (status {}). Your cookies may have expired — try refreshing them.",
                resp.status()
            );
        }

        if final_url.contains("sign_in") {
            bail!(
                "redirected to sign-in page ({}). Your cookies are expired or invalid — re-run the cookie extraction script.",
                final_url
            );
        }

        let body = resp.text().await.context("reading import page body")?;
        let document = Html::parse_document(&body);
        let selector = Selector::parse(r#"meta[name="csrf-token"]"#).expect("valid CSS selector");

        document
            .select(&selector)
            .next()
            .and_then(|el| el.value().attr("content"))
            .map(String::from)
            .context("could not find CSRF token on page — your session may have expired")
    }

    async fn request_export(&self, csrf_token: &str) -> Result<()> {
        let url = format!(
            "{}/review_porter/export/{}",
            GOODREADS_BASE, self.config.user_id
        );
        let referer = format!("{}/review/import", GOODREADS_BASE);
        let resp = self
            .client
            .post(&url)
            .header("X-CSRF-Token", csrf_token)
            .header("X-Requested-With", "XMLHttpRequest")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::REFERER, &referer)
            .header(header::ORIGIN, GOODREADS_BASE)
            .header(header::ACCEPT, "*/*")
            .body("format=json")
            .send()
            .await
            .context("requesting export")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!(
                "export request failed (status {}): {}. Your cookies may have expired.",
                status,
                body
            );
        }

        info!("export request accepted");
        Ok(())
    }

    fn csv_url(&self) -> String {
        format!(
            "{}/review_porter/export/{}/goodreads_export.csv",
            GOODREADS_BASE, self.config.user_id
        )
    }

    async fn poll_until_ready(&self, interval: Duration, max_attempts: u32) -> Result<()> {
        let url = self.csv_url();
        for attempt in 1..=max_attempts {
            let resp = self
                .client
                .head(&url)
                .send()
                .await
                .context("polling export status")?;

            debug!(
                "poll attempt {}/{}: status {}",
                attempt,
                max_attempts,
                resp.status()
            );

            if resp.status().is_success() {
                info!("export ready after {} poll(s)", attempt);
                return Ok(());
            }

            if resp.status() != reqwest::StatusCode::NOT_FOUND {
                bail!(
                    "unexpected status {} while polling for export",
                    resp.status()
                );
            }

            eprint!(".");
            tokio::time::sleep(interval).await;
        }

        bail!(
            "export not ready after {} attempts ({}s total). Try increasing --max-poll-attempts.",
            max_attempts,
            max_attempts as u64 * interval.as_secs()
        );
    }

    async fn download_csv(&self, output: &PathBuf) -> Result<()> {
        let url = self.csv_url();
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("downloading export CSV")?;

        if !resp.status().is_success() {
            bail!("failed to download CSV (status {})", resp.status());
        }

        let bytes = resp.bytes().await.context("reading CSV response body")?;
        tokio::fs::write(output, &bytes)
            .await
            .with_context(|| format!("writing CSV to {}", output.display()))?;

        info!("wrote {} bytes to {}", bytes.len(), output.display());
        Ok(())
    }

    pub async fn export(
        &self,
        output: PathBuf,
        poll_interval: Duration,
        max_poll_attempts: u32,
    ) -> Result<()> {
        eprintln!("Scraping CSRF token...");
        let csrf_token = self.scrape_csrf_token().await?;
        debug!(
            "got CSRF token: {}...",
            &csrf_token[..8.min(csrf_token.len())]
        );

        eprintln!("Requesting export for user {}...", self.config.user_id);
        self.request_export(&csrf_token).await?;

        eprint!("Waiting for export to be ready");
        self.poll_until_ready(poll_interval, max_poll_attempts)
            .await?;
        eprintln!();

        eprintln!("Downloading CSV...");
        self.download_csv(&output).await?;

        eprintln!("Export saved to {}", output.display());
        Ok(())
    }
}
