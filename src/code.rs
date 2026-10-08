//! Syncs git repositories to the code instance, one document per piece.
//!
//! A piece's URL is `vscode://file/<path>:<line>:1`, which is unique per piece
//! and opens VS Code at that line when clicked in hister's web UI. Line numbers
//! shift when a file changes, so a changed file has all its old pieces deleted
//! before the new ones are added. The state file remembers which URLs each
//! file produced.
//!
//! A piece is a hister `code` document. Its `added` is the file's first commit
//! and `updated` its last, or the modification time when the file has
//! uncommitted changes. hister embeds `updated` as the piece's date.

use crate::{chunk, hister};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::process::Command;
use std::path::{Path, PathBuf};

/// Files above this are generated or data, not code worth searching.
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// Raised whenever what a piece carries changes, so that every file is
/// re-added once. A re-added piece whose text is unchanged is not re-embedded.
const PIECE_FORMAT: u32 = 5;

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

/// First and last commit time of every path in the history, relative to the
/// repository root. One `git log` walks the whole history, newest first.
fn commit_times(repo: &Path) -> HashMap<PathBuf, (i64, i64)> {
    let mut times = HashMap::new();
    let Ok(out) = Command::new("git").arg("-C").arg(repo).args(["log", "--format=%x01%ct", "--name-only", "--no-renames"]).output() else {
        return times;
    };
    let mut when = 0;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(t) = line.strip_prefix('\x01') {
            when = t.parse().unwrap_or(0);
        } else if !line.is_empty() {
            let e = times.entry(PathBuf::from(line)).or_insert((when, when));
            e.0 = when;
        }
    }
    times
}

/// Paths with uncommitted changes, including untracked ones.
fn dirty(repo: &Path) -> HashSet<PathBuf> {
    let Ok(out) = Command::new("git").arg("-C").arg(repo).args(["status", "--porcelain", "-z", "--untracked-files=all", "--no-renames"]).output() else {
        return HashSet::new();
    };
    out.stdout.split(|&b| b == 0).filter(|e| e.len() > 3).map(|e| PathBuf::from(String::from_utf8_lossy(&e[3..]).as_ref())).collect()
}

fn url(path: &Path, line: usize, column: usize) -> String {
    format!("vscode://file{}:{line}:{column}", encode(path))
}

// Only the space needs escaping in practice: `~/Projects/linux transition`.
fn encode(path: &Path) -> String {
    path.display().to_string().replace('%', "%25").replace(' ', "%20")
}

/// A hister query filter for the pieces under any of `dirs`, which are
/// absolute: a `url_re` on the start of their URLs. A directory matches only
/// itself, so `repo` leaves out its worktree `repo-branch`.
pub fn under(dirs: &[PathBuf]) -> String {
    // hister's regular expressions are Go's, where a backslash before any
    // ASCII punctuation is a literal.
    let escape = |s: String| s.chars().map(|c| if c.is_ascii_punctuation() && c != '/' { format!("\\{c}") } else { c.to_string() }).collect::<String>();
    let dirs: Vec<String> = dirs.iter().map(|d| escape(encode(&d.components().collect::<PathBuf>()))).collect();
    format!("url_re:\"vscode://file({})/.*\"", dirs.join("|"))
}

/// Each piece's URL, which is its identity in hister. A piece is named by its
/// first line and column 1, or by its real column when an earlier piece of the
/// file starts on the same line: with column 1 for both, the second overwrote
/// the first (880 of 47,039 pieces on 2026-10-06). Column 1 otherwise keeps
/// the URLs of every other piece as they were, so they are not re-embedded.
fn piece_urls(path: &Path, pieces: &[chunk::Piece]) -> Vec<String> {
    let mut lines = HashSet::new();
    pieces
        .iter()
        .map(|p| if lines.insert(p.line) { url(path, p.line, 1) } else { url(path, p.line, p.column) })
        .collect()
}

pub fn index(roots: &[PathBuf]) -> Result<()> {
    let server = hister::server();
    let state_file = state_path();
    // The timer and the VS Code extension both run index-code. One run at a
    // time, since each rewrites the state file.
    std::fs::create_dir_all(state_file.parent().expect("state path has a parent"))?;
    let lock = std::fs::File::create(state_file.with_extension("lock"))?;
    lock.lock()?;
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
        let commits = commit_times(&repo);
        let changed = dirty(&repo);

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

            let rel_path = path.strip_prefix(&repo).unwrap_or(path);
            let mtime = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
            let (added, updated) = match commits.get(rel_path) {
                Some(&(first, last)) if !changed.contains(rel_path) => (first, last),
                Some(&(first, _)) => (first, mtime),
                None => (mtime, mtime),
            };

            // A new date is sent even when the text is unchanged, so results
            // and date filters show it. hister re-embeds only on a text change.
            // Raising PIECE_FORMAT re-adds every piece once.
            let hash: String = Sha256::new()
                .chain_update(PIECE_FORMAT.to_le_bytes())
                .chain_update(text.as_bytes())
                .chain_update(updated.to_le_bytes()).finalize().iter().map(|b| format!("{b:02x}")).collect();
            if state.files.get(path).is_some_and(|f| f.hash == hash) {
                continue;
            }
            if let Some(old) = state.files.get(path) {
                stale.extend(old.urls.iter().cloned());
            }

            let rel = rel_path.display().to_string();
            let ext = path.extension().unwrap_or_default().to_string_lossy().to_lowercase();
            let pieces: Vec<chunk::Piece> = chunk::split(&ext, &text).into_iter().filter(|p| !p.text.trim().is_empty()).collect();
            let urls = piece_urls(path, &pieces);
            for (piece, u) in pieces.into_iter().zip(urls.iter().cloned()) {
                docs.push(hister::Doc {
                    url: u,
                    title: format!("{name}/{rel}:{}", piece.line),
                    text: piece.text,
                    kind: hister::Kind::Code,
                    label: name.clone(),
                    added,
                    updated,
                    metadata: serde_json::Value::Null,
                });
            }
            updates.push((path.to_path_buf(), FileState { hash, urls }));
        }

        if updates.is_empty() {
            continue;
        }
        eprintln!("{name}: {} changed files, {} pieces", updates.len(), docs.len());
        // A URL that comes back is overwritten by the add instead. Deleting it
        // first would re-embed a piece whose text did not change.
        let fresh: HashSet<&String> = updates.iter().flat_map(|(_, f)| &f.urls).collect();
        stale.retain(|u| !fresh.contains(u));
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

#[cfg(test)]
mod tests {
    use super::*;

    // The filter as hister applies it: the whole URL must match.
    fn matches(filter: &str, url: &str) -> bool {
        let re = filter.strip_prefix("url_re:\"").unwrap().strip_suffix('"').unwrap();
        regex::Regex::new(&format!("^(?:{re})$")).unwrap().is_match(url)
    }

    #[test]
    fn under_keeps_a_checkout_and_leaves_out_its_worktrees() {
        let filter = under(&[PathBuf::from("/p/repo/"), PathBuf::from("/p/linux (old) v1.2+")]);
        for path in ["/p/repo/a.rs", "/p/repo/src/b.rs", "/p/linux (old) v1.2+/c.rs"] {
            assert!(matches(&filter, &url(Path::new(path), 3, 1)), "{path} under {filter}");
        }
        for path in ["/p/repo-branch/a.rs", "/p/repo.rs", "/p/other/repo/a.rs", "/p/linux (old) v1x2+/c.rs"] {
            assert!(!matches(&filter, &url(Path::new(path), 3, 1)), "{path} not under {filter}");
        }
    }

    #[test]
    fn pieces_on_one_line_get_their_own_urls() {
        // One 8,000-character line is cut into several pieces, all on line 1.
        let text = "word ".repeat(1600);
        let pieces = chunk::split("txt", &text);
        assert!(pieces.len() > 1);
        assert!(pieces.iter().all(|p| p.line == 1));
        let urls = piece_urls(Path::new("/p/a.txt"), &pieces);
        assert_eq!(urls[0], "vscode://file/p/a.txt:1:1");
        assert_eq!(urls.iter().collect::<HashSet<_>>().len(), urls.len());
        // The column is where the piece starts, so the editor opens there.
        for (p, u) in pieces.iter().zip(&urls).skip(1) {
            assert_eq!(*u, format!("vscode://file/p/a.txt:1:{}", p.column));
            assert_eq!(&text[..].chars().skip(p.column - 1).take(10).collect::<String>(), &p.text.chars().take(10).collect::<String>());
        }
    }

    #[test]
    fn a_piece_alone_on_its_line_keeps_column_one() {
        let text = "fn a() {\n    one();\n}\n\nfn b() {\n    two();\n}\n".repeat(60);
        let pieces = chunk::split("rs", &text);
        let lines: HashSet<usize> = pieces.iter().map(|p| p.line).collect();
        assert_eq!(lines.len(), pieces.len());
        let urls = piece_urls(Path::new("/p/a.rs"), &pieces);
        assert!(urls.iter().all(|u| u.ends_with(":1")));
    }
}
