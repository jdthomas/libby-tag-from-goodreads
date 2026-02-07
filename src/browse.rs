use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use futures::StreamExt;
use serde::Deserialize;
use serde::Serialize;
use tracing::debug;
use tracing::info;
use tracing::warn;

use crate::goodreads;
use crate::libby::BookType;
use crate::libby::LibbyClient;
use crate::libby::SearchOptions;

#[derive(Debug, Serialize)]
pub struct BrowseResult {
    pub title: String,
    pub author: String,
    pub pages: Option<i64>,
    pub goodreads_shelves: Vec<String>,
    pub libby_id: String,
    pub goodreads_id: i64,
    pub is_available: bool,
    pub estimated_wait_days: Option<i64>,
    pub holds_count: Option<i64>,
    pub owned_copies: Option<i64>,
    pub available_copies: Option<i64>,
    pub has_kindle: Option<bool>,
    pub subjects: Vec<String>,
    pub average_rating: Option<f64>,
    pub year_published: Option<i16>,
    pub date_added: String,
    pub private_notes: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct FormatCache {
    entries: HashMap<String, Vec<String>>,
}

impl FormatCache {
    async fn load(path: &PathBuf) -> Self {
        match tokio::fs::read_to_string(path).await {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    async fn save(&self, path: &PathBuf) -> Result<()> {
        let data = serde_json::to_string_pretty(self)?;
        tokio::fs::write(path, data).await?;
        Ok(())
    }
}

pub struct BrowseArgs {
    pub goodreads_export_csv: PathBuf,
    pub card_id: String,
    pub goodreads_shelf: String,
    pub tags: Vec<String>,
    pub min_pages: Option<i64>,
    pub max_pages: Option<i64>,
    pub output: PathBuf,
    pub cache_file: PathBuf,
}

pub async fn browse(args: BrowseArgs, libby_conf_file: PathBuf) -> Result<()> {
    let libby_client = LibbyClient::new(libby_conf_file, args.card_id)
        .await
        .context("client creation")?;
    eprintln!("Client setup: {}", libby_client);

    // 1. Parse Goodreads CSV
    let books = goodreads::get_book_titles_from_goodreads_shelf(
        args.goodreads_export_csv,
        &args.goodreads_shelf,
    )
    .await
    .context("reading goodreads export")?;
    info!(
        "Found {} books on '{}' shelf",
        books.len(),
        args.goodreads_shelf
    );

    // 2. Filter by tags
    let books: Vec<_> = books
        .into_iter()
        .filter(|b| args.tags.iter().all(|tag| b.bookshelves.contains(tag)))
        .collect();
    if !args.tags.is_empty() {
        info!("After tag filter ({:?}): {} books", args.tags, books.len());
    }

    // 3. Filter by page count
    let books: Vec<_> = books
        .into_iter()
        .filter(
            |b| match (args.min_pages, args.max_pages, b.number_of_pages) {
                (Some(min), _, Some(p)) if p < min => false,
                (_, Some(max), Some(p)) if p > max => false,
                _ => true,
            },
        )
        .collect();
    info!("After page filter: {} books", books.len());

    // 4. Search Libby in parallel
    eprintln!("Searching Libby for {} ebooks...", books.len());
    let lc = &libby_client;
    let search_results: Vec<_> = futures::stream::iter(books.iter().map(|book| async move {
        let result = lc
            .search_for_book_details(
                SearchOptions {
                    book_type: BookType::Ebook,
                    deep_search: true,
                    max_results: 24,
                },
                &book.title,
                Some(&book.authors),
            )
            .await;
        (book, result)
    }))
    .buffer_unordered(25)
    .collect()
    .await;

    let mut found: Vec<(&goodreads::BookInfo, crate::libby::LibbySearchResultItem)> = Vec::new();
    let mut not_found = 0usize;
    for (book, result) in search_results {
        match result {
            Ok(item) => found.push((book, item)),
            Err(e) => {
                not_found += 1;
                debug!("Not found in Libby: '{}' -- {:?}", book.title, e);
            }
        }
    }
    eprintln!(
        "Found {} of {} books in Libby ({} not found)",
        found.len(),
        found.len() + not_found,
        not_found
    );

    // 5. Load format cache and fetch missing
    let mut cache = FormatCache::load(&args.cache_file).await;
    let uncached: Vec<&str> = found
        .iter()
        .filter(|(_, item)| !cache.entries.contains_key(&item.id))
        .map(|(_, item)| item.id.as_str())
        .collect();

    if !uncached.is_empty() {
        eprintln!("Fetching format details for {} books...", uncached.len());
        let format_results: Vec<_> = futures::stream::iter(uncached.into_iter().map(|id| {
            let lc = &libby_client;
            async move {
                let formats = lc.get_book_formats(id).await;
                (id.to_string(), formats)
            }
        }))
        .buffer_unordered(10)
        .collect()
        .await;

        for (id, formats) in format_results {
            match formats {
                Ok(f) => {
                    cache.entries.insert(id, f);
                }
                Err(e) => {
                    warn!("Failed to fetch formats for {}: {:?}", id, e);
                }
            }
        }
        cache.save(&args.cache_file).await?;
    }

    // 6. Build results
    let mut results: Vec<BrowseResult> = found
        .into_iter()
        .map(|(book, item)| {
            let formats = cache.entries.get(&item.id);
            let has_kindle = formats.map(|f| f.iter().any(|fmt| fmt == "ebook-kindle"));
            BrowseResult {
                title: item.sort_title,
                author: item.first_creator_name,
                pages: book.number_of_pages,
                goodreads_shelves: book.bookshelves.clone(),
                libby_id: item.id,
                goodreads_id: book.book_id,
                is_available: item.is_available,
                estimated_wait_days: item.estimated_wait_days,
                holds_count: item.holds_count,
                owned_copies: item.owned_copies,
                available_copies: item.available_copies,
                has_kindle,
                subjects: item.subjects.into_iter().map(|s| s.name).collect(),
                average_rating: book.average_rating,
                year_published: book.year_published,
                date_added: book.date_added.clone(),
                private_notes: book.private_notes.clone(),
            }
        })
        .collect();

    // Sort: available first, then by pages ascending
    results.sort_by(|a, b| {
        b.is_available.cmp(&a.is_available).then_with(|| {
            a.pages
                .unwrap_or(i64::MAX)
                .cmp(&b.pages.unwrap_or(i64::MAX))
        })
    });

    let available_count = results.iter().filter(|r| r.is_available).count();
    eprintln!(
        "Generated browse page: {} books ({} available now)",
        results.len(),
        available_count
    );

    // 7. Render and write HTML
    let html = render_html(&results);
    tokio::fs::write(&args.output, html).await?;
    eprintln!("Wrote {}", args.output.display());

    Ok(())
}

fn render_html(results: &[BrowseResult]) -> String {
    let json_data = serde_json::to_string(results).unwrap_or_else(|_| "[]".to_string());
    let available_count = results.iter().filter(|r| r.is_available).count();

    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>browse // libby ebooks</title>
<style>
* {{ box-sizing: border-box; margin: 0; padding: 0; }}
body {{
  background: #0a0a0a;
  color: #b0b0b0;
  font-family: "Berkeley Mono", "SF Mono", "Fira Code", "Cascadia Code", monospace;
  font-size: 13px;
  line-height: 1.5;
  padding: 20px;
}}
a {{ color: #5faf5f; text-decoration: none; }}
a:hover {{ color: #87d787; text-decoration: underline; }}

.header {{
  border-bottom: 1px solid #333;
  padding-bottom: 12px;
  margin-bottom: 16px;
}}
.header h1 {{
  color: #5faf5f;
  font-size: 16px;
  font-weight: normal;
  letter-spacing: 2px;
}}
.header .stats {{
  color: #666;
  margin-top: 4px;
}}
.header .stats span {{ color: #5faf5f; }}

.filters {{
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  align-items: flex-end;
  padding: 12px;
  border: 1px solid #222;
  margin-bottom: 16px;
  background: #0f0f0f;
}}
.filter-group {{ display: flex; flex-direction: column; gap: 2px; }}
.filter-group label {{ color: #666; font-size: 11px; text-transform: uppercase; letter-spacing: 1px; }}
.filter-group input[type="text"],
.filter-group input[type="number"] {{
  background: #1a1a1a;
  border: 1px solid #333;
  color: #b0b0b0;
  font-family: inherit;
  font-size: 13px;
  padding: 4px 8px;
  width: 160px;
}}
.filter-group input:focus {{ border-color: #5faf5f; outline: none; }}
.filter-group input[type="number"] {{ width: 80px; }}

.toggle {{ display: flex; align-items: center; gap: 6px; cursor: pointer; user-select: none; }}
.toggle input {{ accent-color: #5faf5f; }}

.shelf-filters {{ display: flex; flex-wrap: wrap; gap: 6px; }}
.shelf-tag {{
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border: 1px solid #333;
  cursor: pointer;
  user-select: none;
  font-size: 11px;
}}
.shelf-tag.active {{ border-color: #5faf5f; color: #5faf5f; }}
.subject-tag.active {{ border-color: #d7af5f; color: #d7af5f; }}
.rating {{ color: #d7af5f; }}
.collapsible-toggle {{ cursor: pointer; }}
.collapsible-toggle:hover {{ color: #b0b0b0; }}
.collapsible {{ max-height: 500px; transition: max-height 0.3s ease-out; overflow: hidden; }}
.collapsible.collapsed {{ max-height: 0; }}

table {{
  width: 100%;
  border-collapse: collapse;
}}
th {{
  text-align: left;
  color: #666;
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 1px;
  padding: 8px 12px;
  border-bottom: 1px solid #333;
  cursor: pointer;
  user-select: none;
  white-space: nowrap;
}}
th:hover {{ color: #5faf5f; }}
th.sorted {{ color: #5faf5f; }}
td {{
  padding: 6px 12px;
  border-bottom: 1px solid #1a1a1a;
  vertical-align: top;
}}
tr:hover td {{ background: #111; }}

.badge {{
  display: inline-block;
  padding: 1px 6px;
  border: 1px solid #333;
  font-size: 10px;
  margin: 1px 2px;
  color: #888;
}}
.avail {{ color: #5faf5f; }}
.wait {{ color: #d7af5f; }}
.unavail {{ color: #5f5f5f; }}
.kindle {{ color: #d75f5f; }}
.sort-arrow {{ font-size: 10px; margin-left: 4px; }}
.col-hidden {{ display: none; }}
.gear-wrapper {{
  position: relative;
  display: inline-block;
  margin-left: 12px;
}}
.gear-btn {{
  background: none;
  border: 1px solid #333;
  color: #666;
  font-size: 16px;
  cursor: pointer;
  padding: 2px 8px;
  font-family: inherit;
}}
.gear-btn:hover {{ color: #5faf5f; border-color: #5faf5f; }}
.gear-panel {{
  display: none;
  position: absolute;
  top: 100%;
  right: 0;
  background: #141414;
  border: 1px solid #333;
  padding: 8px 12px;
  z-index: 100;
  min-width: 160px;
}}
.gear-panel.open {{ display: block; }}
.gear-panel label {{
  display: block;
  padding: 3px 0;
  cursor: pointer;
  color: #999;
  font-size: 12px;
  white-space: nowrap;
}}
.gear-panel label:hover {{ color: #b0b0b0; }}
.gear-panel input {{ accent-color: #5faf5f; margin-right: 6px; }}
</style>
</head>
<body>

<div class="header">
  <h1>&gt; browse // libby ebooks</h1>
  <div class="stats">
    <span id="shown-count">{total}</span> of {total} books shown
    &middot; <span>{available}</span> available now
    <span class="gear-wrapper">
      <button class="gear-btn" id="gear-btn" title="Column settings">&#9881;</button>
      <div class="gear-panel" id="gear-panel"></div>
    </span>
  </div>
</div>

<div class="filters">
  <div class="filter-group">
    <label>search</label>
    <input type="text" id="search" placeholder="title or author...">
  </div>
  <div class="filter-group">
    <label>pages</label>
    <div style="display:flex;gap:4px;align-items:center;">
      <input type="number" id="min-pages" placeholder="min">
      <span style="color:#444">-</span>
      <input type="number" id="max-pages" placeholder="max">
    </div>
  </div>
  <div class="filter-group">
    <label>&nbsp;</label>
    <label class="toggle"><input type="checkbox" id="avail-only"> available only</label>
  </div>
  <div class="filter-group">
    <label>&nbsp;</label>
    <label class="toggle"><input type="checkbox" id="kindle-only"> kindle only</label>
  </div>
  <div class="filter-group">
    <label>shelves</label>
    <div class="shelf-filters" id="shelf-filters"></div>
  </div>
  <div class="filter-group" style="flex-basis:100%;">
    <label class="collapsible-toggle" id="subjects-toggle">subjects <span id="subjects-arrow">+</span></label>
    <div class="shelf-filters collapsible collapsed" id="subject-filters"></div>
  </div>
</div>

<table>
  <thead>
    <tr>
      <th data-sort="title" data-col="title">title<span class="sort-arrow"></span></th>
      <th data-sort="author" data-col="author">author<span class="sort-arrow"></span></th>
      <th data-sort="pages" data-col="pages">pages<span class="sort-arrow"></span></th>
      <th data-sort="rating" data-col="rating">rating<span class="sort-arrow"></span></th>
      <th data-col="shelves">shelves</th>
      <th data-col="subjects">subjects</th>
      <th data-sort="year" data-col="year">year<span class="sort-arrow"></span></th>
      <th data-sort="added" data-col="added">added<span class="sort-arrow"></span></th>
      <th data-col="notes">notes</th>
      <th data-sort="available" data-col="status">status<span class="sort-arrow"></span></th>
      <th data-col="link">link</th>
    </tr>
  </thead>
  <tbody id="book-table"></tbody>
</table>

<script>
const DATA = {json_data};

let sortCol = "available";
let sortAsc = false;

const allShelves = [...new Set(DATA.flatMap(b => b.goodreads_shelves))].sort();
const allSubjects = [...new Set(DATA.flatMap(b => b.subjects))].sort();

function initShelves() {{
  const el = document.getElementById("shelf-filters");
  el.innerHTML = allShelves.map(s =>
    `<span class="shelf-tag" data-shelf="${{s}}">${{s}}</span>`
  ).join("");
  el.querySelectorAll(".shelf-tag").forEach(t =>
    t.addEventListener("click", () => {{ t.classList.toggle("active"); render(); }})
  );
}}

function initSubjects() {{
  const el = document.getElementById("subject-filters");
  el.innerHTML = allSubjects.map(s =>
    `<span class="shelf-tag subject-tag" data-subject="${{s}}">${{s}}</span>`
  ).join("");
  el.querySelectorAll(".subject-tag").forEach(t =>
    t.addEventListener("click", () => {{ t.classList.toggle("active"); render(); }})
  );
}}

function getActiveShelves() {{
  return [...document.querySelectorAll("#shelf-filters .shelf-tag.active")].map(t => t.dataset.shelf);
}}

function getActiveSubjects() {{
  return [...document.querySelectorAll("#subject-filters .subject-tag.active")].map(t => t.dataset.subject);
}}

function sortData(data) {{
  return data.sort((a, b) => {{
    let va, vb;
    switch (sortCol) {{
      case "title": va = a.title.toLowerCase(); vb = b.title.toLowerCase(); break;
      case "author": va = a.author.toLowerCase(); vb = b.author.toLowerCase(); break;
      case "pages": va = a.pages || 99999; vb = b.pages || 99999; break;
      case "rating": va = a.average_rating || 0; vb = b.average_rating || 0; break;
      case "year": va = a.year_published || 0; vb = b.year_published || 0; break;
      case "added": va = a.date_added; vb = b.date_added; break;
      case "available":
        va = a.is_available ? 0 : (a.estimated_wait_days || 999);
        vb = b.is_available ? 0 : (b.estimated_wait_days || 999);
        break;
      default: return 0;
    }}
    if (va < vb) return sortAsc ? -1 : 1;
    if (va > vb) return sortAsc ? 1 : -1;
    return 0;
  }});
}}

function render() {{
  const search = document.getElementById("search").value.toLowerCase();
  const minP = parseInt(document.getElementById("min-pages").value) || 0;
  const maxP = parseInt(document.getElementById("max-pages").value) || Infinity;
  const availOnly = document.getElementById("avail-only").checked;
  const kindleOnly = document.getElementById("kindle-only").checked;
  const activeShelves = getActiveShelves();
  const activeSubjects = getActiveSubjects();

  let filtered = DATA.filter(b => {{
    if (search && !b.title.toLowerCase().includes(search) && !b.author.toLowerCase().includes(search)) return false;
    if (b.pages && (b.pages < minP || b.pages > maxP)) return false;
    if (availOnly && !b.is_available) return false;
    if (kindleOnly && b.has_kindle !== true) return false;
    if (activeShelves.length > 0 && !activeShelves.every(s => b.goodreads_shelves.includes(s))) return false;
    if (activeSubjects.length > 0 && !activeSubjects.some(s => b.subjects.includes(s))) return false;
    return true;
  }});

  filtered = sortData(filtered);

  document.getElementById("shown-count").textContent = filtered.length;

  document.querySelectorAll("th").forEach(th => {{
    th.classList.toggle("sorted", th.dataset.sort === sortCol);
    const arrow = th.querySelector(".sort-arrow");
    if (arrow) arrow.textContent = th.dataset.sort === sortCol ? (sortAsc ? " \u25B2" : " \u25BC") : "";
  }});

  const tbody = document.getElementById("book-table");
  tbody.innerHTML = filtered.map(b => {{
    const shelves = b.goodreads_shelves.map(s => `<span class="badge">${{s}}</span>`).join("");
    let status;
    if (b.is_available) {{
      status = `<span class="avail">available</span>`;
    }} else if (b.estimated_wait_days != null) {{
      status = `<span class="wait">~${{b.estimated_wait_days}}d wait</span>`;
    }} else {{
      status = `<span class="unavail">waitlist</span>`;
    }}
    if (b.holds_count != null) {{
      status += `<br><span style="color:#555;font-size:11px">${{b.holds_count}} holds / ${{b.owned_copies || "?"}} copies</span>`;
    }}
    if (b.has_kindle === true) {{
      status += `<br><span class="kindle">kindle</span>`;
    }}
    const pages = b.pages != null ? b.pages : `<span style="color:#333">-</span>`;
    const rating = b.average_rating != null
      ? `<span class="rating">${{b.average_rating.toFixed(2)}}</span>`
      : `<span style="color:#333">-</span>`;
    const subjects = b.subjects.map(s => `<span class="badge">${{s}}</span>`).join("");
    const year = b.year_published != null ? b.year_published : `<span style="color:#333">-</span>`;
    const added = b.date_added || `<span style="color:#333">-</span>`;
    const notes = b.private_notes ? b.private_notes : `<span style="color:#333">-</span>`;
    return `<tr>
      <td data-col="title">${{b.title}}</td>
      <td data-col="author">${{b.author}}</td>
      <td data-col="pages">${{pages}}</td>
      <td data-col="rating">${{rating}}</td>
      <td data-col="shelves">${{shelves}}</td>
      <td data-col="subjects">${{subjects}}</td>
      <td data-col="year">${{year}}</td>
      <td data-col="added">${{added}}</td>
      <td data-col="notes">${{notes}}</td>
      <td data-col="status">${{status}}</td>
      <td data-col="link"><a href="https://www.goodreads.com/book/show/${{b.goodreads_id}}" target="_blank">open</a></td>
    </tr>`;
  }}).join("");
  if (typeof applyColVisibility === "function") applyColVisibility();
}}

document.querySelectorAll("th[data-sort]").forEach(th => {{
  th.addEventListener("click", () => {{
    if (sortCol === th.dataset.sort) {{ sortAsc = !sortAsc; }}
    else {{ sortCol = th.dataset.sort; sortAsc = true; }}
    render();
  }});
}});

["search", "min-pages", "max-pages"].forEach(id =>
  document.getElementById(id).addEventListener("input", render)
);
["avail-only", "kindle-only"].forEach(id =>
  document.getElementById(id).addEventListener("change", render)
);

document.getElementById("subjects-toggle").addEventListener("click", () => {{
  const el = document.getElementById("subject-filters");
  const arrow = document.getElementById("subjects-arrow");
  el.classList.toggle("collapsed");
  arrow.textContent = el.classList.contains("collapsed") ? "+" : "\u2212";
}});

const COLUMNS = [
  {{ key: "title", label: "Title", defaultOn: true }},
  {{ key: "author", label: "Author", defaultOn: true }},
  {{ key: "pages", label: "Pages", defaultOn: true }},
  {{ key: "rating", label: "Rating", defaultOn: true }},
  {{ key: "shelves", label: "Shelves", defaultOn: true }},
  {{ key: "subjects", label: "Subjects", defaultOn: false }},
  {{ key: "year", label: "Year", defaultOn: true }},
  {{ key: "added", label: "Added", defaultOn: true }},
  {{ key: "notes", label: "Notes", defaultOn: false }},
  {{ key: "status", label: "Status", defaultOn: true }},
  {{ key: "link", label: "Link", defaultOn: true }},
];
const STORAGE_KEY = "browse-col-visibility";

function loadColVisibility() {{
  try {{
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY));
    if (saved && typeof saved === "object") return saved;
  }} catch (_) {{}}
  return Object.fromEntries(COLUMNS.map(c => [c.key, c.defaultOn]));
}}

let colVisibility = loadColVisibility();

function saveColVisibility() {{
  localStorage.setItem(STORAGE_KEY, JSON.stringify(colVisibility));
}}

function applyColVisibility() {{
  for (const col of COLUMNS) {{
    const hidden = !colVisibility[col.key];
    document.querySelectorAll(`[data-col="${{col.key}}"]`).forEach(el => {{
      el.classList.toggle("col-hidden", hidden);
    }});
  }}
}}

function initGearPanel() {{
  const panel = document.getElementById("gear-panel");
  panel.innerHTML = COLUMNS.map(c => {{
    const checked = colVisibility[c.key] ? "checked" : "";
    return `<label><input type="checkbox" data-col-toggle="${{c.key}}" ${{checked}}> ${{c.label}}</label>`;
  }}).join("");

  panel.querySelectorAll("input[data-col-toggle]").forEach(cb => {{
    cb.addEventListener("change", () => {{
      colVisibility[cb.dataset.colToggle] = cb.checked;
      saveColVisibility();
      applyColVisibility();
    }});
  }});

  document.getElementById("gear-btn").addEventListener("click", (e) => {{
    e.stopPropagation();
    panel.classList.toggle("open");
  }});
  document.addEventListener("click", () => panel.classList.remove("open"));
  panel.addEventListener("click", (e) => e.stopPropagation());
}}

initGearPanel();
initShelves();
initSubjects();
render();
applyColVisibility();
</script>
</body>
</html>
"##,
        total = results.len(),
        available = available_count,
        json_data = json_data,
    )
}
