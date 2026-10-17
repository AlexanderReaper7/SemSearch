//! A hister stand-in that keeps the documents it is sent.

use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct Hister {
    pub docs: HashMap<String, Value>,
    /// Every batch op as `<op> <url>`, in order.
    pub ops: Vec<String>,
}

/// A fake hister on a free port. `/api/batch` adds, gets and deletes
/// documents in memory. `/search` matches nothing and reports as its `total`
/// the number of stored documents whose `type` the query's `type:` filter
/// names.
pub fn hister() -> (String, Arc<Mutex<Hister>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(Mutex::new(Hister::default()));
    let shared = state.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let path = request.split_whitespace().nth(1).unwrap_or_default().to_string();
            let out = if path.starts_with("/api/batch") {
                let body: Value = serde_json::from_slice(&body).unwrap();
                let mut h = shared.lock().unwrap();
                let results: Vec<Value> = body["ops"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|op| {
                        let url = op["url"].as_str().unwrap().to_string();
                        h.ops.push(format!("{} {url}", op["op"].as_str().unwrap()));
                        match op["op"].as_str().unwrap() {
                            "add" => {
                                let mut doc = op.clone();
                                doc.as_object_mut().unwrap().remove("op");
                                h.docs.insert(url, doc);
                                json!({ "status": 200 })
                            }
                            "get" => match h.docs.get(&url) {
                                Some(doc) => json!({ "status": 200, "document": doc }),
                                None => json!({ "status": 404 }),
                            },
                            _ => {
                                h.docs.remove(&url);
                                json!({ "status": 200 })
                            }
                        }
                    })
                    .collect();
                json!({ "results": results })
            } else {
                let query = path.split_once("query=").map(|(_, q)| q).unwrap_or_default();
                let query = percent_decode(query.split('&').next().unwrap_or_default());
                let query: Value = serde_json::from_str(&query).unwrap_or(Value::Null);
                let kind = query["text"].as_str().unwrap_or_default().split_whitespace().find_map(|t| t.strip_prefix("type:")).map(|t| match t {
                    "web" => 0,
                    "file" => 1,
                    _ => 3,
                });
                let h = shared.lock().unwrap();
                let total = h.docs.values().filter(|d| kind.is_none_or(|k| d["type"].as_u64() == Some(k))).count();
                json!({ "documents": [], "semantic_hits": [], "total": total })
            }
            .to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len()).unwrap();
        }
    });
    (url, state)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                out.push(u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap(), 16).unwrap());
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap()
}
