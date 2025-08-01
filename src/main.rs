use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use clap::Subcommand;
use colored::Colorize;
use futures::StreamExt;
use tracing::debug;
use tracing::info;

pub mod goodreads;
pub mod libby;

use goodreads::get_book_titles_from_goodreads;
use goodreads::get_book_titles_from_goodreads_shelf;
use libby::BookType;
use libby::LibbyClient;

#[derive(Subcommand, Debug)]
#[clap(name = "Goodreads shelves to Libby tag")]
enum Commands {
    /// Uses the copy from device login flow to create bearer token.
    Login(LoginArgs),
    /// Takes as input a good reads export csv file, tag name, and
    Gr2lib(GR2LibbyArgs),
    /// List cards that are synced with account
    ListCards,
}

#[derive(Parser, Debug, Clone)]
struct LoginArgs {
    /// Code from libby app's copy to device
    #[clap(long)]
    code: String,
}

#[derive(Parser, Debug, Clone)]
struct GR2LibbyArgs {
    /// The name of the tag in Libby to set
    #[clap(short, long = "tag")]
    tag_name: String,

    /// The card id in Libby to set the tag on
    #[clap(long)]
    card_id: String,

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

    /// The name of the shelf in good reads to filter out. If set will remove
    /// tags for books matching this shelf.
    #[clap(long)]
    goodreads_remove_shelf: Option<String>,

    /// The type of book (audiobook or ebook) in Libby to tag
    #[clap(long, default_value = "audiobook")]
    book_type: BookType,

    /// Include books that your library does not currently have
    #[clap(long)]
    include_unavailable: bool,

    /// Does all the work with the exception of writing the tags to libby
    #[clap(long)]
    dry_run: bool,
}

#[derive(Debug, Parser)]
#[clap(name = "Goodreads shelves to Libby tag")]
struct CommandArgs {
    #[clap(flatten)]
    log_opts: jt_init_logging::LogOptsClap,

    /// Path to save login results json
    #[clap(long, default_value = "./libby_config.json", global = true)]
    libby_conf_file: PathBuf,

    #[command(subcommand)]
    command: Commands,
}
fn normalize_title(input: &str) -> String {
    input
        .chars()
        .filter(|&c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let app_args = CommandArgs::parse();
    jt_init_logging::init_logging(&app_args.log_opts.into());

    match app_args.command {
        Commands::Login(login_args) => {
            let lc = libby::login(login_args.code).await?;
            tokio::fs::write(&app_args.libby_conf_file, lc.to_json()?).await?
        }
        Commands::Gr2lib(command_args) => {
            gr2libby(command_args, app_args.libby_conf_file).await?;
        }
        Commands::ListCards => {
            let cards = libby::get_cards(app_args.libby_conf_file).await?;
            println!("Cards: {:#?}", cards);
        }
    }
    Ok(())
}

enum TagAction {
    Add,
    Remove,
}

async fn gr2libby(command_args: GR2LibbyArgs, libby_conf_file: PathBuf) -> anyhow::Result<()> {
    let libby_client = LibbyClient::new(libby_conf_file, command_args.card_id)
        .await
        .context("client creation")?;

    eprintln!("Client setup: {}", libby_client);
    eprintln!(
        "Will {}tag books (of type {}) from goodreads shelf '{}' with tag '{}'",
        if command_args.dry_run {
            "(dry-run) "
        } else {
            ""
        },
        command_args.book_type,
        command_args.goodreads_shelf,
        command_args.tag_name,
    );
    if let Some(ref remove_shelf) = &command_args.goodreads_remove_shelf {
        eprint!(
            "Will remove tag '{}' from books on the '{}' shelf",
            command_args.tag_name, remove_shelf
        );
    }

    let tag_info = libby_client
        .get_existing_tag_by_name(&command_args.tag_name)
        .await
        .context("get_existing_tag_by_name")?;

    let mut all_goodread_books = get_book_titles_from_goodreads(command_args.goodreads_export_csv)
        .await
        .context("get_book_titles_from_goodreads_shelf")?;

    let goodread_books = all_goodread_books
        .remove(&command_args.goodreads_shelf)
        .with_context(|| {
            format!(
                "shelf '{}' not found in goodreads export",
                command_args.goodreads_shelf
            )
        })?;
    let goodreads_remove_books =
        if let Some(ref remove_shelf) = &command_args.goodreads_remove_shelf {
            all_goodread_books.remove(remove_shelf).with_context(|| {
                format!("shelf '{}' not found in goodreads export", remove_shelf)
            })?
        } else {
            vec![]
        };

    let existing_books = libby_client
        .get_books_for_tag(&tag_info)
        .await
        .context("get_books_for_tag")?;
    let existing_book_titles: HashSet<String> = existing_books
        .iter()
        .map(|b| normalize_title(&b.title))
        .collect();
    let mut existing_book_ids: HashSet<String> =
        existing_books.iter().map(|b| b.libby_id.clone()).collect();
    info!(
        "Found {} existing books ({} titles)",
        existing_book_ids.len(),
        existing_book_titles.len()
    );

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

    let lc = &libby_client;
    let book_type = command_args.book_type;
    let deep_search = command_args.include_unavailable;

    let mut found_books = futures::stream::iter(
        goodread_books
            .iter()
            .filter(|goodreads::BookInfo { title, .. }| {
                if existing_book_titles.contains(&normalize_title(title)) {
                    println!(
                        "{:20} '{}'",
                        "Already tagged (title)".bright_yellow(),
                        title
                    );
                    false
                } else {
                    true
                }
            })
            .map(|book| (TagAction::Add, book))
            .chain(
                goodreads_remove_books
                    .iter()
                    .filter(|goodreads::BookInfo { title, .. }| {
                        // Only keep already tagged books
                        existing_book_titles.contains(&normalize_title(title))
                    })
                    .map(|book| (TagAction::Remove, book)),
            ),
    )
    .map(
        |(action, goodreads::BookInfo { title, authors, .. })| async move {
            let found_book = lc
                .search_for_book_by_title(
                    libby::SearchOptions {
                        book_type,
                        deep_search,
                        max_results: 24,
                    },
                    title,
                    Some(authors),
                )
                .await;
            (action, title, found_book)
        },
    )
    .buffer_unordered(25);
    let mut existing_ct = 0;
    let mut newly_tagged_ct = 0;
    let mut not_found_ct = 0;
    let mut remove_ct = 0;

    while let Some((action, title, found_book)) = found_books.next().await {
        match found_book {
            Ok(book_info) => {
                if existing_book_ids.contains(&book_info.libby_id) {
                    match action {
                        TagAction::Add => {
                            existing_ct += 1;
                            println!(
                                "{:20} '{}'",
                                "Already tagged (id)".yellow(),
                                book_info.title
                            );
                        }
                        TagAction::Remove => {
                            remove_ct += 1;
                            println!("{:20} '{}'", "Removing".green(), book_info.title);
                            if !command_args.dry_run {
                                libby_client
                                    .untag_book_by_overdrive_id(&tag_info, &book_info.libby_id)
                                    .await?;
                            }
                            existing_book_ids.remove(&book_info.libby_id);
                        }
                    }
                } else {
                    match action {
                        TagAction::Add => {
                            newly_tagged_ct += 1;
                            println!("{:20}'{}'", "Tagging".green(), book_info.title);
                            if !command_args.dry_run {
                                libby_client
                                    .tag_book_by_overdrive_id(&tag_info, &book_info.libby_id)
                                    .await?;
                            }
                            existing_book_ids.insert(book_info.libby_id);
                        }
                        TagAction::Remove => {
                            println!(
                                "{:20} '{}'",
                                "Not tagged, skipping remove(id)".bright_yellow(),
                                book_info.title
                            );
                        }
                    }
                }
            }
            Err(e) => {
                not_found_ct += 1;
                println!("{:20} '{}' -- {:?}", "Could not find".red(), title, e);
            }
        }
    }

    println!(
        "Summary: Tagged {}, Existing {}, Not Found {}, Removed {}.",
        newly_tagged_ct, existing_ct, not_found_ct, remove_ct
    );

    Ok(())
}
