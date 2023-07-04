use anyhow::Result;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::PathBuf;
use tracing::debug;

#[derive(Debug)]
pub struct BookInfo {
    pub title: String,
    pub author: String,
    pub isbn: String,
    pub authors: HashSet<String>,
}
impl From<GoodReadsExportRecord> for BookInfo {
    fn from(other: GoodReadsExportRecord) -> Self {
        let authors = other
            .additional_authors
            .split(',')
            .map(|a| a.to_string())
            .chain([other.author.clone()])
            .filter(|a| !a.is_empty())
            .collect();
        Self {
            title: other.title,
            author: other.author,
            isbn: other.ISBN,
            authors,
        }
    }
}
#[allow(dead_code)]
#[allow(non_snake_case)]
#[derive(Deserialize, Debug, Clone)]
struct GoodReadsExportRecord {
    #[serde(alias = "Book Id")]
    book_id: i64,
    #[serde(alias = "Title")]
    title: String,
    #[serde(alias = "Author")]
    author: String,
    #[serde(alias = "Author l-f")]
    author_l_f: String,
    #[serde(alias = "Additional Authors")]
    additional_authors: String,
    ISBN: String,
    ISBN13: String,
    #[serde(alias = "My Rating")]
    my_rating: Option<String>,
    #[serde(alias = "Average Rating")]
    average_rating: String,
    #[serde(alias = "Publisher")]
    publisher: String,
    #[serde(alias = "Binding")]
    binding: String,
    #[serde(alias = "Number of Pages")]
    number_of_pages: Option<i64>,
    #[serde(alias = "Year Published")]
    year_published: Option<i16>,
    #[serde(alias = "Original Publication Year")]
    original_publication_year: Option<i16>,
    #[serde(alias = "Date Read")]
    date_read: Option<String>,
    #[serde(alias = "Date Added")]
    date_added: String,
    #[serde(alias = "Bookshelves")]
    bookshelves: String,
    #[serde(alias = "Bookshelves with positions")]
    bookshelves_with_positions: String,
    #[serde(alias = "Exclusive Shelf")]
    exclusive_shelf: String,
    #[serde(alias = "My Review")]
    my_review: Option<String>,
    #[serde(alias = "Spoiler")]
    spoiler: Option<String>,
    #[serde(alias = "Private Notes")]
    private_notes: Option<String>,
    #[serde(alias = "Read Count")]
    read_count: i64,
    #[serde(alias = "Owned Copies")]
    owned_copies: i64,
}
pub async fn get_book_titles_from_goodreads_shelf(
    file_path: PathBuf,
    shelf_name: &str,
) -> Result<Vec<BookInfo>> {
    let mut rdr = csv::Reader::from_path(file_path)?;
    debug!("heads={:?}", rdr.headers()?);
    Ok(rdr
        .deserialize()
        .filter_map(|r| r.ok()) // TODO: Fail here instead of skipping deserilization problems?
        .filter_map(|record: GoodReadsExportRecord| {
            record.exclusive_shelf.contains(shelf_name).then(|| {
                debug!("{:#?}", record);
                record.into()
            })
        })
        .collect())
}
