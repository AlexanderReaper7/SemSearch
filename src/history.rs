//! Firefox history into hister, as title, URL and visit times.
//!
//! Page content comes only from the browser extension, captured while the page
//! was open. This importer never fetches a page, and it never replaces a
//! document the extension wrote, because that would throw away captured
//! content. See docs/decisions.md, "Web history content only when it comes
//! free".
//!
//! A document the importer owns carries `metadata.source = "firefox-places"`,
//! or has empty text. Only those are rewritten, and only when something
//! changed. Its text is empty: hister embeds the title, URL and first visit
//! as the document's metadata chunk.

use crate::hister;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;

const OWNER: &str = "firefox-places";

struct Entry {
    title: String,
    first: i64,
    last: i64,
    visits: u64,
}

fn read_places(places: &Path) -> Result<Vec<(String, Entry)>> {
    // Firefox holds places.sqlite open with a WAL. A copy of both files reads
    // consistently without touching the live profile.
    let tmp = std::env::temp_dir().join(format!("semsearch-places-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let db = tmp.join("places.sqlite");
    std::fs::copy(places, &db).with_context(|| format!("copy {}", places.display()))?;
    let wal = places.with_extension("sqlite-wal");
    if wal.exists() {
        std::fs::copy(&wal, tmp.join("places.sqlite-wal"))?;
    }

    // visit_date is microseconds.
    let rows: Vec<(String, String, i64, i64, i64)> = {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT p.url, COALESCE(p.title, ''), MIN(v.visit_date) / 1000000, MAX(v.visit_date) / 1000000, COUNT(*)
             FROM moz_places p JOIN moz_historyvisits v ON v.place_id = p.id
             WHERE p.hidden = 0 AND (p.url LIKE 'https://%' OR p.url LIKE 'http://%')
             GROUP BY p.id",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    std::fs::remove_dir_all(&tmp)?;

    // hister drops the fragment and utm_* parameters from web URLs on add, and
    // lookups have to use the URL hister stores. Fragments are stripped here
    // and the visits of every fragment variant merged. URLs with utm
    // parameters are skipped, since matching hister's re-encoding of the query
    // exactly is not worth it for tracking-link duplicates.
    let mut merged: HashMap<String, Entry> = HashMap::new();
    for (url, title, first, last, visits) in rows {
        let url = url.split('#').next().unwrap_or(&url).to_string();
        if url.contains("?utm") || url.contains("&utm") {
            continue;
        }
        let e = merged.entry(url).or_insert(Entry { title: String::new(), first, last, visits: 0 });
        if last >= e.last || e.title.is_empty() {
            e.title = title;
        }
        e.first = e.first.min(first);
        e.last = e.last.max(last);
        e.visits += visits as u64;
    }
    let mut entries: Vec<_> = merged.into_iter().collect();
    entries.sort_by(|a, b| b.1.last.cmp(&a.1.last));
    Ok(entries)
}

fn owned(doc: &Value) -> bool {
    doc["metadata"]["source"].as_str() == Some(OWNER) || doc["text"].as_str().is_none_or(str::is_empty)
}

pub fn import(places: &Path) -> Result<()> {
    let server = hister::server();
    let entries = read_places(places)?;
    eprintln!("{} history entries", entries.len());

    let (mut added, mut unchanged, mut captured) = (0usize, 0usize, 0usize);
    for part in entries.chunks(hister::MAX_BATCH * 10) {
        let urls: Vec<String> = part.iter().map(|(u, _)| u.clone()).collect();
        let mut docs = Vec::new();
        for ((url, e), stored) in part.iter().zip(hister::get(&server, &urls)?) {
            let doc = hister::Doc {
                url: url.clone(),
                title: e.title.clone(),
                text: String::new(),
                kind: hister::Kind::Web,
                label: String::new(),
                added: e.first,
                updated: e.last,
                metadata: json!({ "source": OWNER, "visits": e.visits }),
            };
            match stored {
                Some(s) if !owned(&s) => captured += 1,
                Some(s)
                    if s["title"].as_str() == Some(&doc.title)
                        && s["added"].as_i64() == Some(e.first)
                        && s["updated"].as_i64() == Some(e.last)
                        && s["metadata"] == doc.metadata =>
                {
                    unchanged += 1
                }
                _ => docs.push(doc),
            }
        }
        added += docs.len() - hister::add(&server, &docs)?;
        eprintln!("  {added} written, {unchanged} unchanged, {captured} captured by the extension");
    }
    Ok(())
}
