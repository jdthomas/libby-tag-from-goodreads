use std::collections::HashSet;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::bail;
use anyhow::Context;
use anyhow::Result;
use base64::Engine;
use clap::Parser;
use itertools::Itertools;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderValue;
use reqwest::IntoUrl;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use tracing::debug;

#[derive(Clone, Debug, Parser)]
pub struct LibbyUser {
    /// Card id as known by libbyapp
    #[clap(long)]
    pub card_id: String,

    #[clap(skip)]
    pub library_advantage_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LibbyConfig {
    bearer_token: String,
}
impl LibbyConfig {
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

#[derive(Debug, Deserialize)]
struct CodeClone {
    result: String,
    #[allow(dead_code)]
    chip: String,
}

pub async fn login(code: String) -> Result<LibbyConfig> {
    // Post to /chip to get identity
    let client = LibbyClient::reqwest_client()?;
    let url = "https://sentry.libbyapp.com/chip?c=d%3A16.8.0&s=0";
    let chip: Chip = client
        .post(url)
        .send()
        .await
        .context("libby post requst")?
        .json()
        .await
        .context("libby post response")?;

    // post to /code with json like: {"code":"12345678"} to do login
    let payload = json!({"code": code});
    let url = "https://sentry.libbyapp.com/chip/clone/code";
    let code_clone: CodeClone = client
        .post(url)
        .bearer_auth(&chip.identity)
        .json(&payload)
        .send()
        .await
        .context("libby post requst")?
        .json()
        .await
        .context("libby post response")?;
    debug!("code_clone: {code_clone:?}");
    if code_clone.result != "cloned" {
        bail!("Clone unsuccessful: {code_clone:?}");
    }

    // Post to chip again to get signed in identity
    let url = "https://sentry.libbyapp.com/chip?c=d%3A16.8.0&s=0";
    let chip: Chip = client
        .post(url)
        .bearer_auth(&chip.identity)
        .send()
        .await
        .context("libby post requst")?
        .json()
        .await
        .context("libby post response")?;
    Ok(LibbyConfig {
        bearer_token: chip.identity,
    })
}
async fn chip(client: &reqwest::Client, identity: &str) -> Result<Chip> {
    let url = "https://sentry.libbyapp.com/chip?c=d%3A16.8.0&s=0";
    let chip: Chip = client
        .post(url)
        .bearer_auth(identity)
        .send()
        .await
        .context("libby post requst")?
        .json()
        .await
        .context("libby post response")?;
    Ok(chip)
}

#[derive(Debug)]
pub struct TagInfo {
    pub uuid: String,
    pub name: String,
    pub total_tagged: i64,
}

#[derive(Debug)]
pub struct BookInfo {
    pub libby_id: String,
    pub title: String,
}

#[allow(dead_code)]
#[derive(clap::ValueEnum, Clone, Debug, Copy)]
pub enum BookType {
    Audiobook,
    Ebook,
}

impl std::fmt::Display for BookType {
    // Required method
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        match self {
            Self::Audiobook => write!(f, "audiobook"),
            Self::Ebook => write!(f, "ebook"),
        }
    }
}

fn encode_name(name: &str) -> String {
    base64::engine::general_purpose::STANDARD
        .encode(name.encode_utf16().map(|b| format!("%u{:02X}", b)).join(""))
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Library {
    website_id: String,
    name: String,
}
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct LibbyCard {
    card_id: String,
    advantage_key: String,
    card_name: String,
    library: Library,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct LibbyCardSync {
    cards: Vec<LibbyCard>,
    result: String,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
struct LibbyBookType {
    id: String,
    name: String,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct LibbySearchResultItem {
    is_available: bool,
    // isOwned: bool,
    // ownedCopies: Option<i64>,
    // edition: Option<String>,
    // estimatedWaitDays: i64,
    // holdsCount: i64,
    // holdsRatio: f64,
    id: String,
    // isPreReleaseTitle: bool,
    // availableCopies: i64,
    // starRating: Option<f64>,
    // starRatingCount: Option<i64>,
    first_creator_name: String,
    // subjects: Vec<serde_json::Value>,
    sort_title: String,
    // title: String,
    // subtitle: String,
    #[serde(alias = "type")]
    book_type: LibbyBookType,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct LibbySearchResult {
    items: Vec<LibbySearchResultItem>,
    total_items: i64,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
struct LibbyTagQuery {
    tag: LibbyTag,
}
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
struct LibbySubject {
    id: String,
    name: String,
}
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct LibbyTaggedItem {
    title_id: String,
    title_format: String, // TODO: Enum { audiobook, .. }
    sort_title: String,
    sort_author: String,
    // titleSubjects: Option<Vec<LibbySubject>>, // Fixme: when empty gives `{}` instad of [] cannot parse
}
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct LibbyTag {
    name: String,
    description: Option<String>,
    taggings: Vec<LibbyTaggedItem>,
    uuid: String,
    total_taggings: i64,
}
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
struct LibbyTagList {
    tags: Vec<LibbyTag>,
}

fn fuzzy_author_compare(haystack: &HashSet<String>, needle: &str) -> bool {
    println!("    {} in {:?}?", needle, haystack);
    let lower_haystack = haystack
        .iter()
        .map(|auth| auth.to_lowercase())
        .collect::<HashSet<String>>();
    let lower_needle = needle.to_lowercase();
    lower_haystack
        .iter()
        .map(|x| edit_distance::edit_distance(x, &lower_needle))
        .min()
        .unwrap_or(usize::MAX)
        < 3
    // TOOD: Something fancy
    // lower_haystack.contains(&lower_needle)
}

fn url_for_query(
    library_advantage_key: &str,
    search_opts: SearchOptions,
    title: &str,
) -> Result<reqwest::Url> {
    let book_type = search_opts.book_type.to_string();
    let max_results = search_opts.max_results.to_string();
    let mut url_params = vec![
        ("query", title),
        ("mediaTypes", &book_type),
        ("perPage", &max_results),
        ("page", "1"),
        ("x-client-id", "dewey"),
    ];

    if search_opts.deep_search {
        // Include books the library doesn't currently have
        url_params.push(("show", "all"));
    }
    let url = reqwest::Url::parse_with_params(
        &format!(
            "https://thunder.api.overdrive.com/v2/libraries/{}/media",
            library_advantage_key
        ),
        &url_params,
    )?;
    debug!("uri: {:?}", url);
    Ok(url)
}

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub book_type: BookType,
    pub deep_search: bool,
    pub max_results: usize,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Chip {
    chip: Option<String>,
    identity: String,
    syncable: bool,
    primary: bool,
}

#[allow(dead_code)]
pub struct LibbyClient {
    client: reqwest::Client,
    config: LibbyConfig,
    chip: Chip,
    card: LibbyCard,
}
impl LibbyClient {
    /// Create a new Libby client
    pub async fn new(libby_conf_file: PathBuf, card_id: String) -> Result<Self> {
        let config: LibbyConfig = serde_json::from_str(
            &tokio::fs::read_to_string(libby_conf_file)
                .await
                .context("reading libby config file")?,
        )
        .context("parsing libby config")?;
        let client = Self::reqwest_client()?;
        let chip = chip(&client, &config.bearer_token).await?;
        let card = Self::get_library_card(&client, &chip.identity, &card_id).await?;
        Ok(Self {
            client,
            config,
            chip,
            card,
        })
    }

    /// Helper to create reqwest client with some common defaults
    fn reqwest_client() -> Result<reqwest::Client> {
        let mut headers = HeaderMap::new();
        headers.insert("Origin", HeaderValue::from_static("https://libbyapp.com"));
        headers.insert("Referer", HeaderValue::from_static("https://libbyapp.com/"));
        headers.insert("Sec-Fetch-Dest", HeaderValue::from_static("empty"));
        headers.insert("Sec-Fetch-Mode", HeaderValue::from_static("cors"));
        headers.insert("Sec-Fetch-Site", HeaderValue::from_static("same-site"));
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/58.0.3029.110 Safari/537.3")
            .default_headers(headers)
            .build()?;
        Ok(client)
    }

    pub async fn tag_book_by_overdrive_id(&self, tag_info: &TagInfo, title_id: &str) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_secs();

        let url = format!(
            "https://vandal.libbyapp.com/tag/tag/{}/{}/tagging/{}?enc=1",
            tag_info.uuid,
            encode_name(&tag_info.name),
            title_id
        );
        debug!("~~JT~~: url={:?}", url);

        let data = json!({ "tagging": { "cardId": self.card.card_id, "createTime": now, "titleId": title_id, "websiteId": self.card.library.website_id } });
        debug!("~~JT~~: {:#?}", data.to_string());
        let response = self.make_logged_in_libby_post_request(url, &data).await?;
        debug!("{:#?}", response);
        Ok(())
    }

    pub async fn get_books_for_tag(&self, tag_info: &TagInfo) -> Result<Vec<BookInfo>> {
        let url = format!(
            "https://vandal.libbyapp.com/tag/{}/{}?enc=1&sort=newest&range=0...{}",
            tag_info.uuid,
            encode_name(&tag_info.name),
            tag_info.total_tagged,
        );
        debug!("~~JT~~: URL={url}");

        let response = self
            .make_logged_in_libby_get_request::<LibbyTagQuery, _>(url)
            .await?;

        debug!("{:#?}", response);
        // TODO: Drain
        Ok(response
            .tag
            .taggings
            .iter()
            .map(|tag| BookInfo {
                libby_id: tag.title_id.clone(),
                title: tag.sort_title.clone(),
            })
            .collect::<Vec<BookInfo>>())
    }

    async fn get_library_card(
        client: &reqwest::Client,
        identity: &str,
        card_id: &str,
    ) -> Result<LibbyCard> {
        let url = "https://sentry.libbyapp.com/chip/sync";

        let card_sync: LibbyCardSync = client
            .get(url)
            .bearer_auth(identity)
            .send()
            .await
            .context("libby request")?
            .json()
            .await
            .context("libby request parsing")?;

        debug!("{:#?}", card_sync);
        if card_sync.result != "synchronized" {
            bail!("Unable to sync card: {card_sync:?}");
        }

        card_sync
            .cards
            .into_iter()
            .find(|card| card.card_id == card_id)
            .context("Unable to sync card")
    }

    pub async fn search_for_book_by_title(
        &self,
        search_opts: SearchOptions,
        title: &str,
        authors: Option<&HashSet<String>>,
    ) -> Result<BookInfo> {
        let url = url_for_query(&self.card.advantage_key, search_opts.clone(), title)?;
        let mut response = self
            .make_libby_library_get_request::<LibbySearchResult, _>(url)
            .await?;

        debug!("{:#?}", response);
        // Library search does not handle subtitles well, if we found nothing, lets
        // try with any part of title leading to ':'
        if response.items.is_empty() && title.contains(':') {
            if let Some(t2) = title.split_once(':').map(|(t2, _)| t2) {
                let url = url_for_query(&self.card.advantage_key, search_opts, t2)?;
                response = self
                    .make_libby_library_get_request::<LibbySearchResult, _>(url)
                    .await?;
            }
        }

        response
            .items
            .iter()
            .find(|b| {
                authors.is_none() || fuzzy_author_compare(authors.unwrap(), &b.first_creator_name)
            })
            .map(|b| BookInfo {
                title: b.sort_title.to_string(),
                libby_id: b.id.to_string(),
            })
            .context(format!("Book '{}' not found", title))
    }

    pub async fn get_existing_tag_by_name(&self, name: &str) -> Result<TagInfo> {
        debug!("Here");
        let response = self
            .make_libby_library_get_request::<LibbyTagList, _>("https://vandal.libbyapp.com/tags")
            .await?;
        debug!("Resp: {:#?}", response);

        let found = response
            .tags
            .iter()
            .find(|t| t.name == name)
            .cloned()
            .context("Unable to find tag by name");
        debug!("{:#?}", found);
        found.map(|lt| TagInfo {
            name: lt.name,
            uuid: lt.uuid,
            total_tagged: lt.total_taggings,
        })
    }

    async fn make_logged_in_libby_get_request<T: serde::de::DeserializeOwned, U: IntoUrl>(
        &self,
        url: U,
    ) -> Result<T> {
        self.client
            .get(url)
            .bearer_auth(&self.chip.identity)
            .body("")
            .send()
            .await
            .context("libby request")?
            .json::<T>()
            .await
            .context("libby request parsing")
    }

    async fn make_logged_in_libby_post_request<U: IntoUrl>(
        &self,
        url: U,
        data: &serde_json::Value,
    ) -> Result<String> {
        self.client
            .post(url)
            .bearer_auth(&self.chip.identity)
            .json(&data)
            .send()
            .await
            .context("libby post requst")?
            .text()
            .await
            .context("libby post response")
    }

    async fn make_libby_library_get_request<T: serde::de::DeserializeOwned, U: IntoUrl>(
        &self,
        url: U,
    ) -> Result<T> {
        debug!("{:?}", self.chip);
        let text = self
            .client
            .get(url)
            .bearer_auth(&self.chip.identity)
            .send()
            .await
            .context("library request")?
            .text()
            .await?;
        // .json::<T>()
        // .await
        // .context("library request parsing")
        debug!("resp text: {:?}", text);
        serde_json::from_str(&text).context("library request parsign")
    }
}

impl std::fmt::Display for LibbyClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Libby Client for card {} (id={}). Library: {} (id={},key={})",
            self.card.card_name,
            self.card.card_id,
            self.card.library.name,
            self.card.library.website_id,
            self.card.advantage_key
        )
    }
}

#[cfg(test)]
mod test {
    use super::*;
    fn token() -> String {
        std::env::var("LIBBY_TOKEN").expect("Set LIBBY_TOKEN env var")
    }
    #[test]
    fn test_encode_name() {
        assert_eq!(
            encode_name("👨🏻‍🦲🎧"),
            "JXVEODNEJXVEQzY4JXVEODNDJXVERkZCJXUyMDBEJXVEODNFJXVEREIyJXVEODNDJXVERkE3"
        );
        assert_eq!(encode_name("🔔"), "JXVEODNEJXVERDE0");
    }

    // sentry.libbyapp.com
    #[tokio::test]
    #[ignore]
    async fn test_client_create() {
        let libby_user = LibbyUser {
            card_id: "10534952".to_owned(),
            bearer_token: token(),
            library_advantage_key: None,
        };
        let _libby_client = LibbyClient::new(libby_user).await.expect("create client");
    }

    #[test_log::test(tokio::test)]
    async fn test_query_tags() {
        let tag_name = "👨‍🔬testing".to_owned();
        let libby_user = LibbyUser {
            card_id: "10534952".to_owned(),
            bearer_token: token(),
            library_advantage_key: None,
        };
        let libby_client = LibbyClient::new(libby_user).await.expect("create client");

        let tag_info = libby_client
            .get_existing_tag_by_name(&tag_name)
            .await
            .expect("load tag");
        debug!("tag info: {:#?}", tag_info);
        let existing_books = libby_client
            .get_books_for_tag(&tag_info)
            .await
            .expect("tag as books");
        debug!("books: {:#?}", existing_books);

        // test search
        let title = " The Cuckoo's Egg";
        let authors = HashSet::from_iter(["Cliff Stoll".to_owned()]);
        libby_client
            .search_for_book_by_title(
                SearchOptions {
                    book_type: BookType::Audiobook,
                    deep_search: false,
                    max_results: 24,
                },
                title,
                Some(&authors),
            )
            .await
            .expect("Seaerch okay");
    }
}
