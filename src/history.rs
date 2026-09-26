//! Firefox history into the web instance, as URL and title only.
//!
//! Page content comes only from the browser extension, captured while the page
//! was open. This importer never fetches a page, and it never re-adds a URL the
//! instance already holds, because that would replace captured content with an
//! empty document. See docs/decisions.md, "Web history content only when it
//! comes free".

use crate::{Source, hister};
use anyhow::{Context, Result};
use std::path::Path;

pub fn import(places: &Path) -> Result<()> {
    let server = Source::Web.server();

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

    let rows: Vec<(String, String, i64)> = {
        let conn = rusqlite::Connection::open(&db)?;
        let mut stmt = conn.prepare(
            "SELECT url, COALESCE(title, ''), COALESCE(last_visit_date, 0) FROM moz_places
             WHERE visit_count > 0 AND hidden = 0
               AND (url LIKE 'https://%' OR url LIKE 'http://%')
             ORDER BY last_visit_date DESC",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?
    };
    std::fs::remove_dir_all(&tmp)?;

    // hister drops the fragment and utm_* parameters from web URLs on add. The
    // existence check has to ask for the URL hister stores, or a re-run would
    // overwrite captured content. Fragments are stripped here the same way;
    // URLs with utm parameters are skipped, since matching hister's re-encoding
    // of the query exactly is not worth it for tracking-link duplicates.
    let mut seen = std::collections::HashSet::new();
    let docs: Vec<hister::Doc> = rows
        .into_iter()
        .filter_map(|(url, title, visited)| {
            let url = url.split('#').next().unwrap_or(&url).to_string();
            if url.contains("?utm") || url.contains("&utm") {
                return None;
            }
            seen.insert(url.clone()).then(|| hister::Doc { url, title, text: String::new(), label: String::new(), added: visited / 1_000_000 })
        })
        .collect();
    eprintln!("{} history entries", docs.len());

    let (mut added, mut kept) = (0usize, 0usize);
    for part in docs.chunks(hister::MAX_BATCH * 10) {
        let urls: Vec<String> = part.iter().map(|d| d.url.clone()).collect();
        let missing: Vec<hister::Doc> = part
            .iter()
            .zip(hister::existing(&server, &urls)?)
            .filter(|(_, exists)| !exists)
            .map(|(d, _)| d.clone())
            .collect();
        kept += part.len() - missing.len();
        added += missing.len() - hister::add(&server, &missing)?;
        eprintln!("  {added} added, {kept} already present");
    }
    Ok(())
}
