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

// Deep search gives each tool call a time limit (src/ask.rs, TOOL_TIMEOUT),
// and the count after an empty search is part of the call.
#[test]
fn the_count_after_an_empty_search_keeps_the_time_limit() {
    use std::io::{BufRead, BufReader, Write};
    use std::time::{Duration, Instant};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    // The search answers at once with nothing; the count takes 5 s.
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut request = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut request).unwrap();
            std::thread::spawn(move || {
                if request.contains("limit") {
                    std::thread::sleep(Duration::from_secs(5));
                }
                let out = r#"{"documents":[],"semantic_hits":[],"total":0}"#;
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len());
            });
        }
    });
    let started = Instant::now();
    let search = semsearch::hister::Search { query: "anything", since: None, before: None };
    let found = semsearch::hister::search_within(&server, Some(semsearch::Source::Code), &search, Some(Duration::from_secs(1)));
    let took = started.elapsed();
    assert!(took < Duration::from_secs(3), "took {took:?} with a 1 s limit");
    assert!(found.unwrap().hits.is_empty());
}
