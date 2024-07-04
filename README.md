# libby-tag-from-goodreads

Simple script to take a shelf from a [Goodreads](https://goodreads.com) export and import it into [Libby](https://libbyapp.com) as a tag.

1. [export](https://help.goodreads.com/s/article/How-do-I-import-or-export-my-books-1553870934590) your Goodreads library (their API is apparently deprecated)
2. Build it: `cargo build --release`
3. Open libby on another device, go to settings and [copy to another device](https://help.libbyapp.com/en-us/6070.htm), use that code in the login command: `gr2libby login --code <CODE>` (This will create a libby_config.json with the bearer_token)
4. If you know your library card id, use it, otherwise run `gr2libby list-cards` to see the cards associated with the login.
5. run the script, e.g. `gr2libby gr2lib --card-id $LIBRARY_CARD_ID_FROM_STEP_4 --tag "🎧" --book-type audiobook --goodreads-export-csv $CSV_EXPORT_FROM_STEP_1 --goodreads-shelf "to-read"`
6. ...
7. Profit