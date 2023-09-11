use clap::Parser;
use std::collections::HashSet;
use std::path::PathBuf;
use tracing::{debug, info};
use tracing_subscriber::filter::Directive;

pub mod goodreads;
pub mod libby;
pub mod logging;

use goodreads::get_book_titles_from_goodreads_shelf;

use libby::BookType;
use libby::LibbyClient;
use libby::LibbyUser;

use logging::init_logging;

#[derive(Debug, Parser)]
#[clap(name = "Goodreads shelves to Libby tag")]
struct CommandArgs {
    #[clap(short, long, default_value = "info", hide = true)]
    log: Vec<Directive>,

    #[clap(flatten)]
    libby_user: LibbyUser,

    /// The name of the tag in Libby to set
    #[clap(short, long = "tag")]
    tag_name: String,

    /// Path to local file with a goodreads exported csv.
    /// For information on how to export, see this article:
    ///   https://help.goodreads.com/s/article/How-do-I-import-or-export-my-books-1553870934590
    #[clap(long)]
    goodreads_export_csv: PathBuf,

    /// When set the tagging will be done on the intersection of titles on both
    /// the goodreaeds-export-csv and this second
    /// intersect_with_goodreads_export_csv. This might be useful for creating a
    /// tag for roadtrips with a partner.
    #[clap(long)]
    intersect_with_goodreads_export_csv: Option<PathBuf>,

    /// The name of the shelf in good reads to filter for
    #[clap(long, default_value = "to-read")]
    goodreads_shelf: String,

    /// The type of book (audiobook or ebook) in Libby to tag
    #[clap(long, default_value = "audiobook")]
    book_type: BookType,

    /// Does all the work with the exception of writing the tags to libby
    #[clap(long)]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let command_args = CommandArgs::parse();
    init_logging(command_args.log);

    let libby_client = LibbyClient::new(command_args.libby_user.clone()).await?;

    let tag_info = libby_client
        .get_existing_tag_by_name(&command_args.tag_name)
        .await?;

    let goodread_books = get_book_titles_from_goodreads_shelf(
        command_args.goodreads_export_csv,
        &command_args.goodreads_shelf,
    )
    .await?;

    let existing_books = libby_client.get_books_for_tag(&tag_info).await?;
    let existing_book_titles: HashSet<String> =
        existing_books.iter().map(|b| b.title.clone()).collect();
    let mut existing_book_ids: HashSet<String> =
        existing_books.iter().map(|b| b.libby_id.clone()).collect();
    info!("Found {} existing books", existing_book_ids.len());

    let goodread_books = if let Some(intersect_with_goodreads_export_csv) =
        command_args.intersect_with_goodreads_export_csv
    {
        let intersect_book_titles: HashSet<_> = get_book_titles_from_goodreads_shelf(
            intersect_with_goodreads_export_csv,
            &command_args.goodreads_shelf,
        )
        .await?
        .drain(..)
        .map(|bi| bi.title)
        .collect();
        // Just filter by title
        goodread_books
            .into_iter()
            .filter(|bi| intersect_book_titles.contains(&bi.title))
            .collect()
    } else {
        goodread_books
    };

    debug!("books: {:#?}", goodread_books);
    for goodreads::BookInfo { title, authors, .. } in goodread_books.iter() {
        if existing_book_titles.contains(title) {
            println!("Already tagged '{}'", title);
            continue;
        }
        let found_book = libby_client
            .search_for_book_by_title(command_args.book_type.clone(), title, Some(authors))
            .await;

        match found_book {
            Ok(book_info) => {
                if existing_book_ids.contains(&book_info.libby_id) {
                    println!("Already tagged '{}'", book_info.title);
                } else {
                    println!("Tagging        '{}'", book_info.title);
                    if !command_args.dry_run {
                        libby_client
                            .tag_book_by_overdrive_id(&tag_info, &book_info.libby_id)
                            .await?;
                    }
                    existing_book_ids.insert(book_info.libby_id);
                }
            }
            Err(e) => {
                println!("Could not find '{}' -- {:?}", title, e);
            }
        }
    }

    Ok(())
}
