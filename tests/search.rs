//! `semsearch search` against a fake hister whose searches match nothing.

mod common;

use std::process::Command;

fn search(server: &str, source: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_semsearch")).args(["search", "-s", source, "anything"]).env("SEMSEARCH_URL", server).output().unwrap();
    assert!(out.status.success(), "search: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stderr).unwrap()
}

// The web source was empty for 11 days, and every search of it returned
// nothing as if no page matched.
#[test]
fn a_search_of_an_empty_source_says_it_is_empty() {
    let (server, _) = common::hister();
    let stderr = search(&server, "web");
    assert!(stderr.contains("hister holds no web documents"), "{stderr}");
}

#[test]
fn a_search_of_a_source_with_documents_does_not() {
    let (server, hister) = common::hister();
    hister.lock().unwrap().docs.insert("https://a.example/".into(), serde_json::json!({ "url": "https://a.example/", "type": 0 }));
    let stderr = search(&server, "web");
    assert!(!stderr.contains("holds no"), "{stderr}");
    let stderr = search(&server, "code");
    assert!(stderr.contains("hister holds no code documents"), "{stderr}");
}
