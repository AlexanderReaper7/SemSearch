//! Syncs git repositories to the code instance, one document per piece.
//!
//! A piece's URL is `vscode://file/<path>:<line>:1`, which is unique per piece
//! and opens VS Code at that line when clicked in hister's web UI. Line numbers
//! shift when a file changes, so a changed file has all its old pieces deleted
//! before the new ones are added. The state file remembers which URLs each
//! file produced.

use crate::{Source, chunk, hister};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Files above this are generated or data, not code worth searching.
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// Generated files that are text but never what a search is after.
const SKIP_NAMES: &[&str] = &["Cargo.lock", "flake.lock", "package-lock.json", "pnpm-lock.yaml", "yarn.lock", "uv.lock", "poetry.lock"];

#[derive(Serialize, Deserialize, Default)]
struct State {
    files: BTreeMap<PathBuf, FileState>,
}

#[derive(Serialize, Deserialize)]
struct FileState {
    hash: String,
    urls: Vec<String>,
}

fn state_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME is set")).join(".local/share"));
    base.join("semantic-search/code-state.json")
}

fn repositories(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut repos = Vec::new();
    for root in roots {
        let root = root.canonicalize().with_context(|| format!("{}", root.display()))?;
        if root.join(".git").exists() {
            repos.push(root);
            continue;
        }
        for entry in std::fs::read_dir(&root)? {
            let path = entry?.path();
            if path.join(".git").exists() {
                repos.push(path);
            }
        }
    }
    repos.sort();
    Ok(repos)
}

fn url(path: &Path, line: usize) -> String {
    // Only the space needs escaping in practice: `~/Projects/linux transition`.
    format!("vscode://file{}:{line}:1", path.display().to_string().replace('%', "%25").replace(' ', "%20"))
}

pub fn index(roots: &[PathBuf]) -> Result<()> {
    let server = Source::Code.server();
    let state_file = state_path();
    let mut state: State = match std::fs::read(&state_file) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("code state file")?,
        Err(_) => State::default(),
    };

    let mut seen = HashSet::new();
    let (mut added, mut changed_files) = (0usize, 0usize);
    for repo in repositories(roots)? {
        let name = repo.file_name().unwrap_or_default().to_string_lossy().to_string();
        let mut docs = Vec::new();
        let mut stale = Vec::new();
        let mut updates = Vec::new();

        for entry in ignore::WalkBuilder::new(&repo).build() {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            let file_name = path.file_name().unwrap_or_default().to_string_lossy();
            if SKIP_NAMES.contains(&file_name.as_ref()) || file_name.ends_with(".min.js") {
                continue;
            }
            if entry.metadata().map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(true) {
                continue;
            }
            let Ok(bytes) = std::fs::read(path) else { continue };
            if bytes.contains(&0) {
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else { continue };
            seen.insert(path.to_path_buf());

            let hash: String = Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
            if state.files.get(path).is_some_and(|f| f.hash == hash) {
                continue;
            }
            if let Some(old) = state.files.get(path) {
                stale.extend(old.urls.iter().cloned());
            }

            let rel = path.strip_prefix(&repo).unwrap_or(path).display().to_string();
            let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
            let mut urls = Vec::new();
            for piece in chunk::split(&ext, &text) {
                if piece.text.trim().is_empty() {
                    continue;
                }
                let u = url(path, piece.line);
                urls.push(u.clone());
                docs.push(hister::Doc {
                    url: u,
                    title: format!("{name}/{rel}:{}", piece.line),
                    text: piece.text,
                    label: name.clone(),
                    added: 0,
                });
            }
            updates.push((path.to_path_buf(), FileState { hash, urls }));
        }

        if updates.is_empty() {
            continue;
        }
        eprintln!("{name}: {} changed files, {} pieces", updates.len(), docs.len());
        hister::delete(&server, &stale)?;
        let failed = hister::add(&server, &docs)?;
        added += docs.len() - failed;
        changed_files += updates.len();
        // Saved per repository, so an interrupted run resumes where it stopped.
        state.files.extend(updates);
        save(&state_file, &state)?;
    }

    let gone: Vec<PathBuf> = state
        .files
        .keys()
        .filter(|p| !seen.contains(*p) && roots.iter().any(|r| r.canonicalize().is_ok_and(|r| p.starts_with(r))))
        .cloned()
        .collect();
    if !gone.is_empty() {
        let urls: Vec<String> = gone.iter().flat_map(|p| state.files[p].urls.clone()).collect();
        hister::delete(&server, &urls)?;
        for p in &gone {
            state.files.remove(p);
        }
        save(&state_file, &state)?;
    }
    eprintln!("done: {changed_files} files changed, {added} pieces added, {} files removed", gone.len());
    Ok(())
}

fn save(path: &Path, state: &State) -> Result<()> {
    std::fs::create_dir_all(path.parent().expect("state path has a parent"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(state)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}
