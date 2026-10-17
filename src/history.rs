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
//!
//! A run is incremental. The state file keeps, per places.sqlite, the newest
//! visit the last run read and the URL of the newest entry it imported. The
//! next run sends only entries visited after that visit. When hister no longer
//! holds that URL, the instance has lost its history, as a new data directory
//! did on 2026-10-05, and the run imports everything again.

use crate::hister;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

const OWNER: &str = "firefox-places";

struct Entry {
    title: String,
    first: i64,
    last: i64,
    /// The last visit in microseconds, as Firefox stores it, to compare with
    /// the state file's.
    last_us: i64,
    visits: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    profiles: BTreeMap<PathBuf, Seen>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Seen {
    /// Microseconds: the newest visit in places.sqlite when it was read.
    last_visit: i64,
    /// The newest entry imported so far, to check that hister still holds
    /// what was imported.
    url: String,
}

/// The first bytes of the WAL, or nothing if there is none. Its header holds
/// the salts that change when Firefox starts the WAL over.
fn wal_header(wal: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    match std::fs::File::open(wal) {
        Ok(f) => {
            let mut header = Vec::with_capacity(32);
            f.take(32).read_to_end(&mut header)?;
            Ok(header)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("open {}", wal.display())),
    }
}

/// Copies places.sqlite and its WAL into `dir`, so the copy reads consistently
/// without touching the live profile.
///
/// The database goes first: a checkpoint between the two copies only moves
/// frames the WAL copy still has. A restart of the WAL between them would
/// leave new frames beside a database missing the checkpointed ones, and it
/// rewrites the WAL header, so the copy is retried when the header changed.
fn copy_places(places: &Path, dir: &Path) -> Result<PathBuf> {
    let db = dir.join("places.sqlite");
    let wal = places.with_extension("sqlite-wal");
    for _ in 0..5 {
        let before = wal_header(&wal)?;
        std::fs::copy(places, &db).with_context(|| format!("copy {}", places.display()))?;
        let _ = std::fs::remove_file(dir.join("places.sqlite-wal"));
        if !before.is_empty() {
            std::fs::copy(&wal, dir.join("places.sqlite-wal")).with_context(|| format!("copy {}", wal.display()))?;
        }
        if wal_header(&wal)? == before {
            return Ok(db);
        }
    }
    bail!("{} kept starting its WAL over while it was copied", places.display())
}

/// URL, title, first and last visit in microseconds, and the visit count.
type Row = (String, String, i64, i64, i64);

struct Places {
    /// Every history entry by URL, newest first.
    entries: Vec<(String, Entry)>,
    /// The newest visit of any page, in microseconds.
    newest: Option<i64>,
}

fn read_places(places: &Path) -> Result<Places> {
    let tmp = std::env::temp_dir().join(format!("semsearch-places-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let db = copy_places(places, &tmp)?;

    // visit_date is microseconds.
    let (rows, newest): (Vec<Row>, Option<i64>) = {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT p.url, COALESCE(p.title, ''), MIN(v.visit_date), MAX(v.visit_date), COUNT(*)
             FROM moz_places p JOIN moz_historyvisits v ON v.place_id = p.id
             WHERE p.hidden = 0 AND (p.url LIKE 'https://%' OR p.url LIKE 'http://%')
             GROUP BY p.id",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let newest = conn.query_row("SELECT MAX(visit_date) FROM moz_historyvisits", [], |r| r.get(0))?;
        (rows, newest)
    };
    std::fs::remove_dir_all(&tmp)?;

    // hister drops the fragment and utm_* parameters from web URLs on add, and
    // lookups have to use the URL hister stores. Fragments are stripped here
    // and the visits of every fragment variant merged. URLs with utm
    // parameters are skipped, since matching hister's re-encoding of the query
    // exactly is not worth it for tracking-link duplicates.
    let mut merged: HashMap<String, Entry> = HashMap::new();
    for (url, title, first_us, last_us, visits) in rows {
        let url = url.split('#').next().unwrap_or(&url).to_string();
        if url.contains("?utm") || url.contains("&utm") {
            continue;
        }
        let e = merged.entry(url).or_insert(Entry { title: String::new(), first: i64::MAX, last: 0, last_us: 0, visits: 0 });
        if last_us >= e.last_us || e.title.is_empty() {
            e.title = title;
        }
        e.first = e.first.min(first_us / 1_000_000);
        e.last = e.last.max(last_us / 1_000_000);
        e.last_us = e.last_us.max(last_us);
        e.visits += visits as u64;
    }
    let mut entries: Vec<_> = merged.into_iter().collect();
    entries.sort_by_key(|(_, e)| std::cmp::Reverse(e.last_us));
    Ok(Places { entries, newest })
}

fn owned(doc: &Value) -> bool {
    doc["metadata"]["source"].as_str() == Some(OWNER) || doc["text"].as_str().is_none_or(str::is_empty)
}

pub fn import(places: &Path) -> Result<()> {
    let server = hister::server();
    let state_file = crate::data_file("history-state.json");
    // The timer and a run by hand both rewrite the state file.
    std::fs::create_dir_all(state_file.parent().expect("state path has a parent"))?;
    let lock = std::fs::File::create(state_file.with_extension("lock"))?;
    lock.lock()?;
    let mut state: State = match std::fs::read(&state_file) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("history state file")?,
        Err(_) => State::default(),
    };
    let key = std::path::absolute(places)?;
    let seen = state.profiles.get(&key).cloned();
    let after = match &seen {
        Some(s) if hister::get(&server, std::slice::from_ref(&s.url))?[0].is_some() => Some(s.last_visit),
        Some(s) => {
            eprintln!("hister lacks {}, which the last import added: importing all history again", s.url);
            None
        }
        None => None,
    };

    let Places { entries, newest } = read_places(places)?;
    let total = entries.len();
    let entries: Vec<_> = entries.into_iter().filter(|(_, e)| after.is_none_or(|a| e.last_us > a)).collect();
    match after {
        Some(_) => eprintln!("{total} history entries, {} visited since the last import", entries.len()),
        None => eprintln!("{total} history entries"),
    }

    let (mut added, mut unchanged, mut captured, mut failed) = (0usize, 0usize, 0usize, 0usize);
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
        let f = hister::add(&server, &docs)?;
        failed += f;
        added += docs.len() - f;
        eprintln!("  {added} written, {unchanged} unchanged, {captured} captured by the extension");
    }

    // A failed add is sent again by the next run, which starts from the old
    // visit.
    if failed > 0 {
        bail!("{failed} entries failed to add; the next run tries them again");
    }
    let url = entries.first().map(|(u, _)| u.clone()).or(seen.map(|s| s.url));
    if let (Some(url), Some(last_visit)) = (url, newest) {
        state.profiles.insert(key, Seen { last_visit, url });
        let tmp = state_file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&state)?)?;
        std::fs::rename(tmp, &state_file)?;
    }
    Ok(())
}
