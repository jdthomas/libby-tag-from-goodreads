# libby-tag-from-goodreads

Simple script to take a shelf from a [Goodreads](https://goodreads.com) export and import it into [Libby](https://libbyapp.com) as a tag.

1. [export](https://help.goodreads.com/s/article/How-do-I-import-or-export-my-books-1553870934590) your Goodreads library (their API is apparently deprecated)
2. Login to Libby and use the browser debug tools network tab to find teh 'Authorization' header and grab the bearer token
3. run the script, e.g. `gr2libby --card-id $_LIBRARY_CARD_ID --bearer-token $TOKEN_FROM_STEP_2 --tag "🎧" --book-type audiobook --goodreads-export-csv $CSV_EXPORT_FROM_STEP_1 --goodreads-shelf "to-read"`
4. ...
5. Profit