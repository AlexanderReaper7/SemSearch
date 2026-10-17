//! `semsearch import-history` against a fake hister that keeps what it is sent.

mod common;

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::Command;

const DAY_US: i64 = 86_400 * 1_000_000;
/// 2026-10-01 00:00 UTC, in microseconds.
const T0: i64 = 1_790_812_800 * 1_000_000;

fn scratch(name: &str) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("semsearch-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    tmp
}

/// A places.sqlite with the columns the importer reads, in WAL mode as
/// Firefox keeps it.
fn places(dir: &Path) -> (PathBuf, Connection) {
    let path = dir.join("places.sqlite");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT UNIQUE, title TEXT, hidden INTEGER NOT NULL DEFAULT 0);
         CREATE TABLE moz_historyvisits (id INTEGER PRIMARY KEY, place_id INTEGER, visit_date INTEGER);",
    )
    .unwrap();
    (path, conn)
}

fn visit(conn: &Connection, url: &str, title: &str, at: i64) {
    conn.execute("INSERT OR IGNORE INTO moz_places (url, title) VALUES (?1, ?2)", (url, title)).unwrap();
    conn.execute("INSERT INTO moz_historyvisits (place_id, visit_date) SELECT id, ?2 FROM moz_places WHERE url = ?1", (url, at)).unwrap();
}

fn run_import(server: &str, data: &Path, places: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_semsearch")).arg("import-history").arg(places).env("SEMSEARCH_URL", server).env("XDG_DATA_HOME", data).output().unwrap()
}

fn import(server: &str, data: &Path, places: &Path) {
    let out = run_import(server, data, places);
    assert!(out.status.success(), "import-history: {}", String::from_utf8_lossy(&out.stderr));
}

fn ops(h: &std::sync::Mutex<common::Hister>, op: &str) -> Vec<String> {
    let prefix = format!("{op} ");
    let mut urls: Vec<String> = h.lock().unwrap().ops.iter().filter_map(|o| o.strip_prefix(&prefix).map(str::to_string)).collect();
    urls.sort();
    urls
}

// A timer runs the import, so a run looks up only what changed since the last
// one rather than all of the history.
#[test]
fn a_second_run_sends_only_entries_visited_since_the_first() {
    let tmp = scratch("history-incremental");
    let (path, conn) = places(&tmp);
    visit(&conn, "https://a.example/", "A", T0);
    visit(&conn, "https://b.example/", "B", T0 + DAY_US);
    visit(&conn, "https://c.example/", "C", T0 + 2 * DAY_US);
    let (server, hister) = common::hister();
    let data = tmp.join("data");
    import(&server, &data, &path);
    assert_eq!(ops(&hister, "add").len(), 3);
    hister.lock().unwrap().ops.clear();

    visit(&conn, "https://b.example/", "B", T0 + 3 * DAY_US);
    visit(&conn, "https://d.example/", "D", T0 + 4 * DAY_US);
    import(&server, &data, &path);
    assert_eq!(ops(&hister, "add"), ["https://b.example/", "https://d.example/"]);
    // c is the check that hister still holds the last import.
    assert_eq!(ops(&hister, "get"), ["https://b.example/", "https://c.example/", "https://d.example/"]);
    let h = hister.lock().unwrap();
    let b = &h.docs["https://b.example/"];
    assert_eq!(b["metadata"]["visits"], 2, "{b}");
    assert_eq!(b["added"], (T0 + DAY_US) / 1_000_000, "{b}");
    assert_eq!(b["updated"], (T0 + 3 * DAY_US) / 1_000_000, "{b}");
    drop(conn);
    let _ = std::fs::remove_dir_all(&tmp);
}

// On 2026-10-05 a new data directory left hister without the history it had
// been sent. A state file alone would have kept saying it was imported.
#[test]
fn an_instance_that_lost_its_history_gets_all_of_it_again() {
    let tmp = scratch("history-lost");
    let (path, conn) = places(&tmp);
    visit(&conn, "https://a.example/", "A", T0);
    visit(&conn, "https://b.example/", "B", T0 + DAY_US);
    let data = tmp.join("data");
    let (server, _) = common::hister();
    import(&server, &data, &path);

    let (empty, hister) = common::hister();
    import(&empty, &data, &path);
    assert_eq!(ops(&hister, "add"), ["https://a.example/", "https://b.example/"]);
    drop(conn);
    let _ = std::fs::remove_dir_all(&tmp);
}

// Firefox keeps recent visits in places.sqlite-wal until it checkpoints.
#[test]
fn visits_still_in_the_wal_are_imported() {
    let tmp = scratch("history-wal");
    let (path, conn) = places(&tmp);
    conn.execute_batch("PRAGMA wal_autocheckpoint = 0;").unwrap();
    visit(&conn, "https://a.example/", "A", T0);
    assert!(tmp.join("places.sqlite-wal").metadata().unwrap().len() > 0);
    let (server, hister) = common::hister();
    import(&server, &tmp.join("data"), &path);
    assert_eq!(ops(&hister, "add"), ["https://a.example/"]);
    drop(conn);
    let _ = std::fs::remove_dir_all(&tmp);
}

// Firefox Sync and history imports insert visits dated before visits that
// are already there.
#[test]
fn a_visit_dated_before_the_last_import_is_still_imported() {
    let tmp = scratch("history-backdated");
    let (path, conn) = places(&tmp);
    visit(&conn, "https://a.example/", "A", T0 + 5 * DAY_US);
    let (server, hister) = common::hister();
    let data = tmp.join("data");
    import(&server, &data, &path);
    hister.lock().unwrap().ops.clear();

    visit(&conn, "https://b.example/", "B", T0 + DAY_US);
    import(&server, &data, &path);
    assert_eq!(ops(&hister, "add"), ["https://b.example/"]);
    drop(conn);
    let _ = std::fs::remove_dir_all(&tmp);
}

// moz_historyvisits has no AUTOINCREMENT, so after its newest visit is
// deleted, as clearing recent history does, the next visit gets the same id.
#[test]
fn a_visit_that_reuses_a_deleted_id_is_imported() {
    let tmp = scratch("history-reused-id");
    let (path, conn) = places(&tmp);
    visit(&conn, "https://a.example/", "A", T0);
    visit(&conn, "https://b.example/", "B", T0 + DAY_US);
    let (server, hister) = common::hister();
    let data = tmp.join("data");
    import(&server, &data, &path);
    let top: i64 = conn.query_row("SELECT MAX(id) FROM moz_historyvisits", [], |r| r.get(0)).unwrap();
    conn.execute("DELETE FROM moz_historyvisits WHERE id = ?1", [top]).unwrap();
    hister.lock().unwrap().ops.clear();

    visit(&conn, "https://c.example/", "C", T0 + 2 * DAY_US);
    let reused: i64 = conn.query_row("SELECT MAX(id) FROM moz_historyvisits", [], |r| r.get(0)).unwrap();
    assert_eq!(reused, top);
    import(&server, &data, &path);
    assert!(ops(&hister, "add").contains(&"https://c.example/".to_string()), "{:?}", ops(&hister, "add"));
    drop(conn);
    let _ = std::fs::remove_dir_all(&tmp);
}
