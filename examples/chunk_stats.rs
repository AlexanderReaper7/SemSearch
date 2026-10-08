//! How src/chunk.rs cuts real files, next to text-splitter's CodeSplitter
//! (what chunk.rs used until 2026-10-07) and the blank-line TextSplitter that
//! files without a grammar get. See bench/README.md, "Chunking".
//!
//!     cargo run --release --example chunk_stats -- [--ext rs,py] PATH...
//!     cargo run --release --example chunk_stats -- --dump FILE
//!     cargo run --release --example chunk_stats -- --tree FILE   node kinds, for LEADING
//!
//! Each metric is measured against the grammar's own parse of the file:
//! - needless: a cut inside a named node whose lines, from the start of its
//!   first to the end of its last, would have fit in one piece. Lines, not the
//!   node alone: a body that fits but starts on the line of a signature that
//!   does not fit with it cannot stay whole either way.
//! - no blank: a cut at the start of a line with no blank line above it.
//!   Code puts blank lines between definitions, so this counts the cuts the
//!   tree cannot see: between a signature and its equations, between the
//!   clauses of one function, inside a block of line comments.
//! - detached: a cut right after a comment that sits directly above the code
//!   that follows.
//! - mid-line: a cut with code before it on the same line.
//! - <400 and <100: pieces with too little in them to stand alone.

#[path = "../src/chunk.rs"]
#[allow(dead_code)]
mod chunk;

use std::path::{Path, PathBuf};
use std::time::Instant;
use text_splitter::{ChunkConfig, CodeSplitter};
use tree_sitter::{Language, Node};

/// The same limit as index-code (src/code.rs).
const MAX_FILE_BYTES: u64 = 512 * 1024;

#[derive(Default)]
struct Split {
    pieces: usize,
    sizes: Vec<usize>,
    cuts: usize,
    needless: usize,
    no_blank: usize,
    detached: usize,
    mid_line: usize,
    secs: f64,
}

#[derive(Default)]
struct Group {
    files: usize,
    bytes: usize,
    error_files: usize,
    error_bytes: usize,
    /// Files whose parse ran past chunk.rs's PARSE_BUDGET. chunk.rs cuts
    /// them as text, CodeSplitter would hang on them, and there is no tree to
    /// measure against, so they count here and nowhere else.
    given_up: usize,
    worst: Vec<(f64, PathBuf)>,
    splits: [Split; 3],
}

const SPLITTERS: [&str; 3] = ["chunk.rs", "code", "text"];

/// The grammars index-code gave CodeSplitter until 2026-10-07, by first
/// extension. Every other language went to the text splitter, so the code row
/// is left out for it. CodeSplitter also has inputs it never finishes: a 365 KB
/// gperf header in ghostty ran past 600 s, where chunk.rs takes 1.3 s.
const CODESPLITTER: &[&str] = &["rs", "py", "ts", "tsx", "js", "cs", "java", "go", "nix", "sh"];

fn walk(path: &Path, out: &mut Vec<PathBuf>) {
    let Ok(meta) = path.symlink_metadata() else { return };
    if meta.is_dir() {
        if path.file_name().is_some_and(|n| n == ".git") {
            return;
        }
        let Ok(rd) = std::fs::read_dir(path) else { return };
        let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for e in entries {
            walk(&e, out);
        }
    } else if meta.is_file() && meta.len() <= MAX_FILE_BYTES {
        out.push(path.to_path_buf());
    }
}

fn ext(path: &Path) -> String {
    path.extension().unwrap_or_default().to_string_lossy().to_lowercase()
}

fn read(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn error_bytes(node: Node) -> usize {
    if node.is_error() {
        return node.end_byte() - node.start_byte();
    }
    let mut c = node.walk();
    node.children(&mut c).map(error_bytes).sum()
}

fn chars(text: &str, n: Node) -> usize {
    text[n.start_byte()..n.end_byte()].chars().count()
}

/// The lines `n` spans fit in one piece.
fn lines_fit(text: &str, n: Node) -> bool {
    let start = text[..n.start_byte()].rfind('\n').map_or(0, |i| i + 1);
    let end = text[n.end_byte()..].find('\n').map_or(text.len(), |i| n.end_byte() + i);
    text[start..end].trim().chars().count() < chunk::SIZE.end
}

/// Named nodes with text on both sides of byte `o`.
fn containing<'a>(root: Node<'a>, text: &str, o: usize) -> Vec<Node<'a>> {
    let mut out = Vec::new();
    let mut node = root;
    'down: loop {
        let mut c = node.walk();
        for n in node.children(&mut c) {
            if !text[n.start_byte().min(o)..o].trim().is_empty() && o < n.end_byte() {
                if n.is_named() {
                    out.push(n);
                }
                node = n;
                continue 'down;
            }
        }
        return out;
    }
}

fn is_comment(kind: &str) -> bool {
    // Haskell's grammar calls its doc comments `haddock`.
    kind.contains("comment") || kind == "haddock"
}

/// The cut at `o` separates a comment from the code directly below it.
fn detached(root: Node, text: &str, o: usize) -> bool {
    let p = text[..o].trim_end().len();
    if p == 0 || text[p..o].matches('\n').count() >= 2 {
        return false;
    }
    let Some(mut n) = root.descendant_for_byte_range(p - 1, p - 1) else { return false };
    loop {
        if is_comment(n.kind()) {
            return true;
        }
        match n.parent() {
            Some(parent) if parent.end_byte() == n.end_byte() && parent.id() != root.id() => n = parent,
            _ => return false,
        }
    }
}

fn mid_line(text: &str, o: usize) -> bool {
    let line_start = text[..o].rfind('\n').map_or(0, |i| i + 1);
    !text[line_start..o].trim().is_empty()
}

fn no_blank(text: &str, o: usize) -> bool {
    let p = text[..o].trim_end().len();
    !mid_line(text, o) && text[p..o].matches('\n').count() < 2
}

fn measure(split: &mut Split, root: Node, text: &str, pieces: &[(usize, &str)]) {
    for &(o, piece) in pieces {
        if piece.trim().is_empty() {
            continue;
        }
        split.pieces += 1;
        split.sizes.push(piece.chars().count());
        if text[..o].trim().is_empty() {
            continue;
        }
        split.cuts += 1;
        split.needless += usize::from(containing(root, text, o).iter().any(|n| lines_fit(text, *n)));
        split.no_blank += usize::from(no_blank(text, o));
        split.detached += usize::from(detached(root, text, o));
        split.mid_line += usize::from(mid_line(text, o));
    }
}

fn pct(a: usize, b: usize) -> f64 {
    if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 }
}

fn print(name: &str, g: &mut Group) {
    let mb = g.bytes as f64 / 1e6;
    println!(
        "{name}  files {}  {mb:.2} MB  parse given up: {}  parse errors: {} files ({:.1}%), {:.2}% of bytes",
        g.files,
        g.given_up,
        g.error_files,
        pct(g.error_files, g.files),
        pct(g.error_bytes, g.bytes),
    );
    println!("  splitter   pieces  median   <400   <100  needless  no-blank  detached  mid-line  ms/MB");
    for (label, s) in SPLITTERS.iter().zip(&mut g.splits) {
        if s.pieces == 0 {
            continue;
        }
        s.sizes.sort_unstable();
        let under = |n: usize| pct(s.sizes.iter().filter(|&&c| c < n).count(), s.pieces);
        println!(
            "  {label:9} {:7} {:7} {:5.1}% {:5.1}% {:8.1}% {:8.1}% {:8.1}% {:8.1}% {:6.0}",
            s.pieces,
            s.sizes.get(s.sizes.len() / 2).copied().unwrap_or(0),
            under(400),
            under(100),
            pct(s.needless, s.cuts),
            pct(s.no_blank, s.cuts),
            pct(s.detached, s.cuts),
            pct(s.mid_line, s.cuts),
            if mb > 0.0 { s.secs * 1000.0 / mb } else { 0.0 },
        );
    }
    g.worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (frac, path) in g.worst.iter().take(3).filter(|w| w.0 > 0.0) {
        println!("  {:5.1}% in ERROR  {}", frac * 100.0, path.display());
    }
}

fn dump(path: &Path) {
    let text = read(path).expect("a UTF-8 text file");
    let ext = ext(path);
    let g = chunk::grammar(&ext).expect("an extension with a grammar");
    let tree = chunk::parse(g, &text).expect("a parse within PARSE_BUDGET");
    let root = tree.root_node();
    for (o, piece) in chunk::offsets(&ext, &text) {
        let around: Vec<String> = containing(root, &text, o).iter().map(|n| format!("{}({})", n.kind(), chars(&text, *n))).collect();
        let mut flags = String::new();
        let needless = containing(root, &text, o).iter().any(|n| lines_fit(&text, *n));
        for (on, flag) in [(needless, "NEEDLESS"), (detached(root, &text, o), "DETACHED"), (mid_line(&text, o), "MID-LINE"), (no_blank(&text, o), "NO-BLANK")] {
            if on && !text[..o].trim().is_empty() {
                flags += &format!(", {flag}");
            }
        }
        println!(
            "--- line {}, {} chars, inside [{}]{flags}",
            text[..o].matches('\n').count() + 1,
            piece.chars().count(),
            around.join(" > "),
        );
        let lines: Vec<&str> = piece.lines().collect();
        for l in lines.iter().take(3) {
            println!("  | {l}");
        }
        if lines.len() > 3 {
            if lines.len() > 4 {
                println!("  | ...");
            }
            println!("  | {}", lines[lines.len() - 1]);
        }
    }
}

/// Named nodes down to depth 4, with their lines.
fn tree(path: &Path) {
    let text = read(path).expect("a UTF-8 text file");
    let g = chunk::grammar(&ext(path)).expect("an extension with a grammar");
    let tree = chunk::parse(g, &text).expect("a parse within PARSE_BUDGET");
    fn walk(n: Node, depth: usize) {
        if n.is_named() {
            println!("{}{} {}-{}", "  ".repeat(depth), n.kind(), n.start_position().row + 1, n.end_position().row + 1);
        }
        let mut c = n.walk();
        for child in n.children(&mut c).filter(|_| depth < 4) {
            walk(child, depth + 1);
        }
    }
    walk(tree.root_node(), 0);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut exts: Option<Vec<String>> = None;
    let mut roots = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dump" => return dump(Path::new(&args.next().expect("--dump FILE"))),
            "--tree" => return tree(Path::new(&args.next().expect("--tree FILE"))),
            "--ext" => exts = Some(args.next().expect("--ext list").split(',').map(str::to_lowercase).collect()),
            _ => roots.push(PathBuf::from(a)),
        }
    }
    assert!(!roots.is_empty(), "usage: chunk_stats [--ext rs,py] PATH... | --dump FILE");

    let mut files = Vec::new();
    for r in &roots {
        walk(r, &mut files);
    }
    let mut groups: Vec<Group> = chunk::GRAMMARS.iter().map(|_| Group::default()).collect();
    let splitters: Vec<Option<CodeSplitter<_>>> = chunk::GRAMMARS
        .iter()
        .map(|g| {
            CODESPLITTER
                .contains(&g.extensions[0])
                .then(|| CodeSplitter::new(Language::new(g.language), ChunkConfig::new(chunk::SIZE)).expect("CodeSplitter"))
        })
        .collect();
    for path in &files {
        let ext = ext(path);
        if exts.as_ref().is_some_and(|e| !e.contains(&ext)) {
            continue;
        }
        let Some(i) = chunk::GRAMMARS.iter().position(|g| g.extensions.contains(&ext.as_str())) else { continue };
        let Some(text) = read(path) else { continue };
        let code = &splitters[i];
        let g = &mut groups[i];
        g.files += 1;
        g.bytes += text.len();
        let Some(tree) = chunk::parse(&chunk::GRAMMARS[i], &text) else {
            g.given_up += 1;
            continue;
        };
        let root = tree.root_node();
        if root.has_error() {
            let eb = error_bytes(root);
            g.error_files += 1;
            g.error_bytes += eb;
            g.worst.push((eb as f64 / text.len().max(1) as f64, path.clone()));
        }
        let start = Instant::now();
        let ours = chunk::offsets(&ext, &text);
        g.splits[0].secs += start.elapsed().as_secs_f64();
        let start = Instant::now();
        let theirs: Vec<_> = code.as_ref().map_or(Vec::new(), |c| c.chunk_indices(&text).collect());
        g.splits[1].secs += start.elapsed().as_secs_f64();
        let start = Instant::now();
        let plain = chunk::plain(&text);
        g.splits[2].secs += start.elapsed().as_secs_f64();
        for (split, pieces) in g.splits.iter_mut().zip([&ours, &theirs, &plain]) {
            if !pieces.is_empty() {
                measure(split, root, &text, pieces);
            }
        }
    }
    for (g, group) in chunk::GRAMMARS.iter().zip(&mut groups) {
        if group.files > 0 {
            print(g.extensions[0], group);
        }
    }
}
