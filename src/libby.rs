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

const C: &str = "d:18.4.0";
const V: &str = "eb643ccd";
const S: &str = "0";

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
    let url = format!("https://sentry.libbyapp.com/chip?c={C}&s={S}&v={V}");
    let chip: Chip = client
        .post(url)
        .send()
        .await
        .context("libby post request")?
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
        .context("libby post request")?
        .json()
        .await
        .context("libby post response")?;
    if code_clone.result != "cloned" {
        bail!("Clone unsuccessful: {code_clone:?}");
    }

    // Post to chip again to get signed in identity
    let url = format!("https://sentry.libbyapp.com/chip?c={C}&s={S}&v={V}");
    let chip: Chip = client
        .post(url)
        .bearer_auth(&chip.identity)
        .send()
        .await
        .context("libby post request")?
        .json()
        .await
        .context("libby post response")?;
    Ok(LibbyConfig {
        bearer_token: chip.identity,
    })
}

async fn chip(client: &reqwest::Client, identity: &str) -> Result<Chip> {
    let url = format!("https://sentry.libbyapp.com/chip?c={C}&s={S}&v={V}");
    let chip_resp = client
        .post(url)
        .bearer_auth(identity)
        .send()
        .await
        .context("libby post request")?
        .text()
        .await
        .context("get resp")?;
    debug!("Resp: '{}'", chip_resp);
    let chip: Chip = serde_json::from_str(&chip_resp).context("libby post response")?;
    Ok(chip)
}

pub async fn get_cards(libby_conf_file: PathBuf) -> Result<Vec<LibbyCard>> {
    let client = LibbyClient::reqwest_client()?;
    let libby_config = LibbyClient::load_config(libby_conf_file).await?;
    let cards = LibbyClient::get_cards(&client, &libby_config.bearer_token).await?;
    Ok(cards)
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
pub struct Library {
    pub website_id: String,
    pub name: String,
}
#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LibbyCard {
    pub card_id: String,
    pub advantage_key: String,
    pub card_name: String,
    pub library: Library,
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
pub(crate) struct LibbyBookType {
    pub id: String,
    pub name: String,
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LibbySearchResultItem {
    pub is_available: bool,
    pub is_owned: Option<bool>,
    pub owned_copies: Option<i64>,
    pub estimated_wait_days: Option<i64>,
    pub holds_count: Option<i64>,
    pub available_copies: Option<i64>,
    pub id: String,
    pub first_creator_name: String,
    pub sort_title: String,
    #[serde(alias = "type")]
    pub book_type: LibbyBookType,
    #[serde(default, deserialize_with = "deserialize_subjects")]
    pub subjects: Vec<LibbySubject>,
}

fn deserialize_subjects<'de, D>(deserializer: D) -> std::result::Result<Vec<LibbySubject>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // OverDrive API returns {} instead of [] when empty, so we handle both
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Array(arr) => {
            let mut subjects = Vec::new();
            for item in arr {
                if let Ok(subject) = serde_json::from_value::<LibbySubject>(item) {
                    subjects.push(subject);
                }
            }
            Ok(subjects)
        }
        _ => Ok(Vec::new()),
    }
}

#[allow(dead_code)]
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct LibbyResult {
    result: String,
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
pub(crate) struct LibbySubject {
    pub id: String,
    pub name: String,
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
        let config = Self::load_config(libby_conf_file)
            .await
            .context("load config")?;
        let client = Self::reqwest_client()?;
        let chip = chip(&client, &config.bearer_token).await.context("Chip")?;
        let card = Self::get_library_card(&client, &chip.identity, &card_id)
            .await
            .context("get_library_card")?;
        Ok(Self {
            client,
            config,
            chip,
            card,
        })
    }

    async fn load_config(libby_conf_file: PathBuf) -> Result<LibbyConfig> {
        let config: LibbyConfig = serde_json::from_str(
            &tokio::fs::read_to_string(libby_conf_file)
                .await
                .context("reading libby config file")?,
        )
        .context("parsing libby config")?;
        Ok(config)
    }

    /// Helper to create reqwest client with some common defaults
    fn reqwest_client() -> Result<reqwest::Client> {
        let mut headers = HeaderMap::new();
        headers.insert("Origin", HeaderValue::from_static("https://libbyapp.com"));
        headers.insert("Referer", HeaderValue::from_static("https://libbyapp.com/"));
        headers.insert("Sec-Fetch-Dest", HeaderValue::from_static("empty"));
        headers.insert("Sec-Fetch-Mode", HeaderValue::from_static("cors"));
        headers.insert("Sec-Fetch-Site", HeaderValue::from_static("same-site"));
        headers.insert("Accept", HeaderValue::from_static("application/json"));
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
            "https://vandal.libbyapp.com/tag/{}/{}/tagging/{}?enc=1",
            tag_info.uuid,
            encode_name(&tag_info.name),
            title_id
        );
        let data = json!({ "tagging": { "cardId": self.card.card_id, "createTime": now, "titleId": title_id, "websiteId": self.card.library.website_id } });
        let response: LibbyResult = self.make_logged_in_libby_post_request(url, &data).await?;
        if response.result != "created" {
            bail!("Unable to tag book: {response:?}");
        }
        debug!("{:#?}", response);
        Ok(())
    }
    pub async fn untag_book_by_overdrive_id(
        &self,
        tag_info: &TagInfo,
        title_id: &str,
    ) -> Result<()> {
        let url = format!(
            "https://vandal.libbyapp.com/tag/{}/{}/tagging/{}?enc=1",
            tag_info.uuid,
            encode_name(&tag_info.name),
            title_id
        );
        let response: LibbyResult = self.make_logged_in_libby_delete_request(url).await?;
        if response.result != "taggings_destroyed" {
            bail!("Unable to untag book: {response:?}");
        }
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

    async fn get_cards(client: &reqwest::Client, identity: &str) -> Result<Vec<LibbyCard>> {
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

        Ok(card_sync.cards)
    }

    async fn get_library_card(
        client: &reqwest::Client,
        identity: &str,
        card_id: &str,
    ) -> Result<LibbyCard> {
        let cards = Self::get_cards(client, identity).await?;
        cards
            .into_iter()
            .find(|card| card.card_id == card_id)
            .context("Unable to sync card")
    }

    async fn search_items(
        &self,
        search_opts: SearchOptions,
        title: &str,
        authors: Option<&HashSet<String>>,
    ) -> Result<Option<LibbySearchResultItem>> {
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

        Ok(response.items.into_iter().find(|b| {
            authors.is_none() || fuzzy_author_compare(authors.unwrap(), &b.first_creator_name)
        }))
    }

    pub async fn search_for_book_by_title(
        &self,
        search_opts: SearchOptions,
        title: &str,
        authors: Option<&HashSet<String>>,
    ) -> Result<BookInfo> {
        self.search_items(search_opts, title, authors)
            .await?
            .map(|b| BookInfo {
                title: b.sort_title.to_string(),
                libby_id: b.id.to_string(),
            })
            .context(format!("Book '{}' not found", title))
    }

    pub(crate) async fn search_for_book_details(
        &self,
        search_opts: SearchOptions,
        title: &str,
        authors: Option<&HashSet<String>>,
    ) -> Result<LibbySearchResultItem> {
        self.search_items(search_opts, title, authors)
            .await?
            .context(format!("Book '{}' not found", title))
    }

    pub(crate) async fn get_book_formats(&self, libby_id: &str) -> Result<Vec<String>> {
        let url = format!(
            "https://thunder.api.overdrive.com/v2/libraries/{}/media/{}",
            self.card.advantage_key, libby_id
        );
        let response: serde_json::Value = self.make_libby_library_get_request(url).await?;
        let formats = response
            .get("formats")
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|f| f.get("id").and_then(|id| id.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Ok(formats)
    }

    pub async fn get_existing_tag_by_name(&self, name: &str) -> Result<TagInfo> {
        let response = self
            .make_libby_library_get_request::<LibbyTagList, _>("https://vandal.libbyapp.com/tags")
            .await?;
        let found = response
            .tags
            .iter()
            .find(|t| t.name == name)
            .cloned()
            .context("Unable to find tag by name");
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

    async fn make_logged_in_libby_post_request<T: serde::de::DeserializeOwned, U: IntoUrl>(
        &self,
        url: U,
        data: &serde_json::Value,
    ) -> Result<T> {
        self.client
            .post(url)
            .bearer_auth(&self.chip.identity)
            .json(&data)
            .send()
            .await
            .context("libby post request")?
            .json::<T>()
            .await
            .context("libby post response")
    }

    async fn make_logged_in_libby_delete_request<T: serde::de::DeserializeOwned, U: IntoUrl>(
        &self,
        url: U,
    ) -> Result<T> {
        self.client
            .delete(url)
            .bearer_auth(&self.chip.identity)
            .send()
            .await
            .context("libby post request")?
            .json::<T>()
            .await
            .context("libby post response")
    }

    async fn make_libby_library_get_request<T: serde::de::DeserializeOwned, U: IntoUrl>(
        &self,
        url: U,
    ) -> Result<T> {
        self.client
            .get(url)
            .bearer_auth(&self.chip.identity)
            .send()
            .await
            .context("library request")?
            .json::<T>()
            .await
            .context("library request parsing")
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
    fn libby_config_path() -> PathBuf {
        std::env::var("LIBBY_CONFIG")
            .expect("Set LIBBY_CONFIG env var")
            .into()
    }
    fn card_id() -> String {
        std::env::var("LIBBY_CARD_ID").expect("Set LIBBY_CARD_ID env var")
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
        let libby_conf_file = libby_config_path();
        let card_id = card_id();
        let _libby_client = LibbyClient::new(libby_conf_file, card_id)
            .await
            .expect("create client");
    }

    #[test_log::test(tokio::test)]
    #[ignore]
    async fn test_query_tags() {
        let tag_name = "👨‍🔬testing".to_owned();
        let libby_conf_file = libby_config_path();
        let card_id = card_id();
        let libby_client = LibbyClient::new(libby_conf_file, card_id)
            .await
            .expect("create client");

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
