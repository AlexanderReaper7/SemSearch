//! `semsearch index-code` against a fake hister that records every batch op.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

/// A hister stand-in on a free port. Every op of every `/api/batch` request it
/// accepts lands in the returned list as `<op> <url>`, and every op succeeds.
/// After `accept` requests, if given, it answers 500 and records nothing.
fn fake_hister(accept: Option<usize>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let ops = Arc::new(Mutex::new(Vec::new()));
    let seen = ops.clone();
    std::thread::spawn(move || {
        for (n, stream) in listener.incoming().enumerate() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
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
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let batch = body["ops"].as_array().unwrap();
            if accept.is_some_and(|accept| n >= accept) {
                write!(stream, "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
                continue;
            }
            for op in batch {
                seen.lock().unwrap().push(format!("{} {}", op["op"].as_str().unwrap(), op["url"].as_str().unwrap()));
            }
            let results: Vec<_> = batch.iter().map(|_| serde_json::json!({ "status": 200 })).collect();
            let out = serde_json::json!({ "results": results }).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len()).unwrap();
        }
    });
    (url, ops)
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git").arg("-C").arg(repo).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).status().unwrap();
    assert!(status.success(), "git {args:?}");
}

fn run_index_code(server: &str, data: &Path, root: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_semsearch")).arg("index-code").arg(root).env("SEMSEARCH_URL", server).env("XDG_DATA_HOME", data).output().unwrap()
}

fn index_code(server: &str, data: &Path, root: &Path) {
    let out = run_index_code(server, data, root);
    assert!(out.status.success(), "index-code {}: {}", root.display(), String::from_utf8_lossy(&out.stderr));
}

fn deletes(ops: &Arc<Mutex<Vec<String>>>) -> usize {
    ops.lock().unwrap().iter().filter(|op| op.starts_with("delete ")).count()
}

// VS Code runs index-code on its open folder, which can be a directory inside
// a repository rather than the repository itself.
#[test]
fn a_folder_inside_a_repository_deletes_nothing() {
    let tmp = std::env::temp_dir().join(format!("semsearch-index-code-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let repo = tmp.join("repo");
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    std::fs::write(repo.join("top.rs"), "fn top() {}\n").unwrap();
    std::fs::write(repo.join("sub/b.rs"), "fn b() {}\n").unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "x"]);
    let (server, ops) = fake_hister(None);
    let data = tmp.join("data");

    index_code(&server, &data, &repo);
    assert_eq!(ops.lock().unwrap().iter().filter(|op| op.starts_with("add ")).count(), 2, "{:?}", ops.lock().unwrap());
    ops.lock().unwrap().clear();

    index_code(&server, &data, &repo.join("sub"));
    let ops = ops.lock().unwrap();
    assert!(ops.iter().all(|op| !op.starts_with("delete ")), "{ops:?}");
    let _ = std::fs::remove_dir_all(&tmp);
}

// hister pays for every delete, so a run stopped while deleting the pieces of
// gone files does not send the deletes it already made again.
#[test]
fn a_stopped_run_resumes_its_deletes() {
    let tmp = std::env::temp_dir().join(format!("semsearch-index-code-resume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let repo = tmp.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    const FILES: usize = 150;
    for i in 0..FILES {
        std::fs::write(repo.join(format!("f{i}.rs")), format!("fn f{i}() {{}}\n")).unwrap();
    }
    git(&repo, &["init", "-q"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "x"]);
    let data = tmp.join("data");
    let (server, _) = fake_hister(None);
    index_code(&server, &data, &repo);
    for i in 0..FILES {
        std::fs::remove_file(repo.join(format!("f{i}.rs"))).unwrap();
    }

    // The first delete batch goes through, then hister fails and the run stops.
    let (failing, first) = fake_hister(Some(1));
    assert!(!run_index_code(&failing, &data, &repo).status.success());
    let first = deletes(&first);
    assert!(first > 0 && first < FILES, "first run deleted {first} of {FILES}");

    let (server, second) = fake_hister(None);
    index_code(&server, &data, &repo);
    assert_eq!(first + deletes(&second), FILES, "the second run sent {} deletes after the first sent {first}", deletes(&second));
    let _ = std::fs::remove_dir_all(&tmp);
}
