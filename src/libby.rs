use std::collections::HashSet;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use anyhow::Context;
use anyhow::Result;
use base64::Engine;
use clap::Parser;
use itertools::Itertools;
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

    // pub card_pin: String,
    /// Open libbyapp.com in your browser of choice and after logging in w/ a
    /// library card, use the browser's debug tools to find the value of the
    /// 'Authorization' header as part of any request
    #[clap(long)]
    pub bearer_token: String,

    #[clap(skip)]
    pub library_advantage_key: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LibbyConfig {
    bearer_token: String,
}
pub async fn prepare_user(path: &str, mut libby_user: LibbyUser) -> Result<LibbyUser> {
    let config = tokio::fs::read_to_string(path).await?;
    let config: LibbyConfig = serde_json::from_str(&config)?;
    libby_user.bearer_token = config.bearer_token;
    if libby_user.bearer_token.is_empty() {
        // TODO: prompt to copy from another device
        anyhow::bail!("empty bearer token");
    }
    Ok(libby_user)
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
#[allow(non_snake_case)]
#[derive(Deserialize, Debug, Clone)]
struct LibbyCard {
    cardId: String,
    advantageKey: String,
    cardName: String,
    // limits:
}
#[allow(dead_code)]
#[allow(non_snake_case)]
#[derive(Deserialize, Debug, Clone)]
struct LibbyCardSync {
    cards: Vec<LibbyCard>,
    // holds: Vec<LibbySearchResultItem>,
    // loans: Vec<LibbySearchResultItem>,
    // result: String,
    // summary: BTreeMap<String, _>, // {cards: "done", holds: "done", loans: "done"}
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
struct LibbyBookType {
    id: String,
    name: String,
}

#[allow(dead_code)]
#[allow(non_snake_case)]
#[derive(Deserialize, Debug, Clone)]
struct LibbySearchResultItem {
    isAvailable: bool,
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
    firstCreatorName: String,
    // subjects: Vec<serde_json::Value>,
    sortTitle: String,
    // title: String,
    // subtitle: String,
    #[serde(alias = "type")]
    book_type: LibbyBookType,
}

#[allow(dead_code)]
#[allow(non_snake_case)]
#[derive(Deserialize, Debug, Clone)]
struct LibbySearchResult {
    items: Vec<LibbySearchResultItem>,
    totalItems: i64,
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
#[allow(non_snake_case)]
#[derive(Deserialize, Debug, Clone)]
struct LibbyTaggedItem {
    titleId: String,
    titleFormat: String, // TODO: Enum { audiobook, .. }
    sortTitle: String,
    sortAuthor: String,
    // titleSubjects: Option<Vec<LibbySubject>>, // Fixme: when empty gives `{}` instad of [] cannot parse
}
#[allow(dead_code)]
#[allow(non_snake_case)]
#[derive(Deserialize, Debug, Clone)]
struct LibbyTag {
    name: String,
    description: Option<String>,
    taggings: Vec<LibbyTaggedItem>,
    uuid: String,
    totalTaggings: i64,
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
    libby_user: &LibbyUser,
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
            libby_user
                .library_advantage_key
                .as_ref()
                .expect("Must have library key set to search")
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
#[allow(non_snake_case)]
#[derive(Deserialize, Debug, Clone)]
pub struct Chip {
    chip: Option<String>,
    identity: String,
    syncable: bool,
    primary: bool,
}

pub struct LibbyClient {
    client: reqwest::Client,
    libby_user: LibbyUser,
    chip: Option<Chip>,
}
impl LibbyClient {
    pub async fn new(libby_user: LibbyUser) -> Result<Self> {
        let mut client = Self {
            client: reqwest::Client::builder()
            .user_agent(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:109.0) Gecko/20100101 Firefox/114.0",
            )
            .build()?,
            libby_user,
            chip: None,
        };

        client.update_chip().await?;

        let library_advantage_key = client.get_library_info_for_card().await?;
        client.libby_user.library_advantage_key = Some(library_advantage_key);

        Ok(client)
    }

    async fn update_chip(&mut self) -> Result<()> {
        let url = "https://sentry.libbyapp.com/chip?c=d%3A16.8.0&s=0";

        let resp = self
            .client
            .post(url)
            .header("Origin", "https://libbyapp.com")
            .header("Referer", "https://libbyapp.com")
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-site")
            .bearer_auth(self.libby_user.bearer_token.clone())
            .send()
            .await
            .context("libby post requst")?
            .text()
            .await
            .context("libby post response")?;
        self.chip = Some(serde_json::from_str(&resp)?);

        Ok(())
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

        let data = json!({ "tagging": { "cardId": self.libby_user.card_id, "createTime": now, "titleId": title_id, "websiteId": "83" } });
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
                libby_id: tag.titleId.clone(),
                title: tag.sortTitle.clone(),
            })
            .collect::<Vec<BookInfo>>())
    }

    async fn get_library_info_for_card(&self) -> Result<String> {
        let url = "https://sentry.libbyapp.com/chip/sync";

        let response = self
            .make_logged_in_libby_get_request::<LibbyCardSync, _>(url)
            .await?;

        debug!("{:#?}", response);

        response
            .cards
            .iter()
            .find(|card| card.cardId == self.libby_user.card_id)
            .map(|card| card.advantageKey.clone())
            .context("Unable to sync card")
    }

    pub async fn search_for_book_by_title(
        &self,
        search_opts: SearchOptions,
        title: &str,
        authors: Option<&HashSet<String>>,
    ) -> Result<BookInfo> {
        let url = url_for_query(&self.libby_user, search_opts.clone(), title)?;
        let mut response = self
            .make_libby_library_get_request::<LibbySearchResult, _>(url)
            .await?;

        debug!("{:#?}", response);
        // Library search does not handle subtitles well, if we found nothing, lets
        // try with any part of title leading to ':'
        if response.items.is_empty() && title.contains(':') {
            if let Some(t2) = title.split_once(':').map(|(t2, _)| t2) {
                let url = url_for_query(&self.libby_user, search_opts, t2)?;
                response = self
                    .make_libby_library_get_request::<LibbySearchResult, _>(url)
                    .await?;
            }
        }

        response
            .items
            .iter()
            .find(|b| {
                authors.is_none() || fuzzy_author_compare(authors.unwrap(), &b.firstCreatorName)
            })
            .map(|b| BookInfo {
                title: b.sortTitle.to_string(),
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
            total_tagged: lt.totalTaggings,
        })
    }

    async fn make_logged_in_libby_get_request<T: serde::de::DeserializeOwned, U: IntoUrl>(
        &self,
        url: U,
    ) -> Result<T> {
        self.client
            .get(url)
            .header("Origin", "https://libbyapp.com")
            .header("Referer", "https://libbyapp.com/")
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-site")
            .bearer_auth(self.libby_user.bearer_token.clone())
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
            .header("Origin", "https://libbyapp.com")
            .header("Referer", "https://libbyapp.com/")
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-site")
            .bearer_auth(self.libby_user.bearer_token.clone())
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
            .header("Origin", "https://libbyapp.com")
            .header("Referer", "https://libbyapp.com/")
            .header("Sec-Fetch-Dest", "empty")
            .header("Sec-Fetch-Mode", "cors")
            .header("Sec-Fetch-Site", "same-site")
            .bearer_auth(self.libby_user.bearer_token.clone())
            .send()
            .await
            .context("library request")?
            .text()
            .await?;
        // .json::<T>()
        // .await
        // .context("library request parsing")
        debug!("resp text: {:?}", text);
        Ok(serde_json::from_str(&text).context("library request parsign")?)
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
