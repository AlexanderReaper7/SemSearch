//! Cuts a source file into pieces that hister embeds whole.
//!
//! hister passes text under `semantic_search.max_context_length` through as a
//! single chunk, so a piece cut here is exactly one vector there. Code splits
//! along its syntax tree ([`code`]), Markdown along headings, anything else
//! along blank lines.

use std::collections::HashSet;
use std::ops::{ControlFlow, Range};
use text_splitter::{ChunkConfig, MarkdownSplitter, TextSplitter};
use tree_sitter::{Language, Node, ParseOptions, ParseState, Parser, Tree};
use tree_sitter_language::LanguageFn;

/// Characters per piece. The upper bound stays under the instances'
/// max_context_length of 1024 (hister counts words, and code runs at roughly
/// 3-4 characters per word); the lower bound merges tiny items such as `use`
/// lines into their neighbours.
pub const SIZE: Range<usize> = 400..3000;

/// Progress callbacks a parse may take per KiB of text before it is given up
/// and the file is cut as text. Across 30,170 files a parse took 4 per KiB at
/// the median and 239 at most (a generated C table); the exponential case in
/// tree-sitter-objc issue #22 passed 141,000 per KiB and was still going. A
/// count, not a clock, so a file cuts the same way on a busy machine.
const PARSE_BUDGET: usize = 500;

/// Node kinds that belong to the code below them when they start their own
/// line: doc comments, Rust attributes, decorators, annotations.
const LEADING: &[&str] = &["comment", "attribute", "decorator", "annotation"];

/// Endings of node kinds that close the code above them, like an anonymous
/// `}` does: Svelte's `{/if}` (`if_end`), an HTML end tag, a heredoc's
/// terminator.
const TRAILING: &[&str] = &["_end", "end_tag"];

pub struct Grammar {
    pub extensions: &'static [&'static str],
    pub language: LanguageFn,
    /// Kinds that lead the code below them in this language only, on top of
    /// [`LEADING`].
    pub leading: &'static [&'static str],
}

pub const GRAMMARS: &[Grammar] = &[
    Grammar { extensions: &["rs"], language: tree_sitter_rust::LANGUAGE, leading: &[] },
    Grammar { extensions: &["py"], language: tree_sitter_python::LANGUAGE, leading: &[] },
    Grammar { extensions: &["ts", "mts", "cts"], language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT, leading: &[] },
    Grammar { extensions: &["tsx"], language: tree_sitter_typescript::LANGUAGE_TSX, leading: &[] },
    Grammar { extensions: &["js", "mjs", "cjs", "jsx"], language: tree_sitter_javascript::LANGUAGE, leading: &[] },
    Grammar { extensions: &["cs"], language: tree_sitter_c_sharp::LANGUAGE, leading: &[] },
    Grammar { extensions: &["java"], language: tree_sitter_java::LANGUAGE, leading: &[] },
    Grammar { extensions: &["go"], language: tree_sitter_go::LANGUAGE, leading: &[] },
    Grammar { extensions: &["nix"], language: tree_sitter_nix::LANGUAGE, leading: &[] },
    Grammar { extensions: &["sh", "bash"], language: tree_sitter_bash::LANGUAGE, leading: &[] },
    Grammar { extensions: &["c"], language: tree_sitter_c::LANGUAGE, leading: &[] },
    Grammar { extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx", "h"], language: tree_sitter_cpp::LANGUAGE, leading: &[] },
    Grammar { extensions: &["m"], language: tree_sitter_objc::LANGUAGE, leading: &[] },
    Grammar { extensions: &["php"], language: tree_sitter_php::LANGUAGE_PHP, leading: &[] },
    Grammar { extensions: &["rb", "rake", "gemspec", "ru"], language: tree_sitter_ruby::LANGUAGE, leading: &[] },
    Grammar { extensions: &["kt", "kts"], language: tree_sitter_kt::LANGUAGE, leading: &[] },
    Grammar { extensions: &["swift"], language: tree_sitter_swift::LANGUAGE, leading: &[] },
    Grammar { extensions: &["dart"], language: tree_sitter_dart::LANGUAGE, leading: &[] },
    Grammar { extensions: &["zig"], language: tree_sitter_zig::LANGUAGE, leading: &[] },
    Grammar { extensions: &["lua", "rockspec"], language: tree_sitter_lua::LANGUAGE, leading: &[] },
    Grammar { extensions: &["ex", "exs"], language: tree_sitter_elixir::LANGUAGE, leading: &["unary_operator"] },
    Grammar { extensions: &["erl", "hrl", "escript"], language: tree_sitter_erlang::LANGUAGE, leading: &["spec"] },
    Grammar { extensions: &["clj", "cljs", "cljc", "cljd"], language: tree_sitter_clojure_orchard::LANGUAGE, leading: &[] },
    Grammar { extensions: &["hs"], language: tree_sitter_haskell::LANGUAGE, leading: &["haddock", "signature"] },
    Grammar { extensions: &["ml"], language: tree_sitter_ocaml::LANGUAGE_OCAML, leading: &[] },
    Grammar { extensions: &["mli"], language: tree_sitter_ocaml::LANGUAGE_OCAML_INTERFACE, leading: &[] },
    Grammar { extensions: &["ps1", "psm1"], language: tree_sitter_pwsh::LANGUAGE, leading: &[] },
    Grammar { extensions: &["svelte"], language: tree_sitter_svelte_ng::LANGUAGE, leading: &[] },
    Grammar { extensions: &["gd"], language: tree_sitter_gdscript::LANGUAGE, leading: &[] },
];

pub struct Piece {
    /// 1-based line of the first character.
    pub line: usize,
    /// 1-based column of the first character, in characters.
    pub column: usize,
    pub text: String,
}

pub fn grammar(ext: &str) -> Option<&'static Grammar> {
    GRAMMARS.iter().find(|g| g.extensions.contains(&ext))
}

pub fn split(ext: &str, text: &str) -> Vec<Piece> {
    let mut line = 1;
    let mut counted = 0;
    offsets(ext, text)
        .into_iter()
        .map(|(offset, piece)| {
            line += text[counted..offset].matches('\n').count();
            counted = offset;
            let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
            let column = text[line_start..offset].chars().count() + 1;
            Piece { line, column, text: piece.to_string() }
        })
        .collect()
}

/// Each piece's byte offset and text.
pub fn offsets<'t>(ext: &str, text: &'t str) -> Vec<(usize, &'t str)> {
    if let Some(g) = grammar(ext) {
        code(g, text, SIZE)
    } else if ext == "md" || ext == "markdown" {
        MarkdownSplitter::new(ChunkConfig::new(SIZE)).chunk_indices(text).collect()
    } else {
        plain(text)
    }
}

pub fn plain(text: &str) -> Vec<(usize, &str)> {
    plain_in(text, SIZE)
}

fn plain_in(text: &str, limit: Range<usize>) -> Vec<(usize, &str)> {
    TextSplitter::new(ChunkConfig::new(limit)).chunk_indices(text).collect()
}

/// The syntax tree, or `None` if the parse runs past [`PARSE_BUDGET`].
pub fn parse(grammar: &Grammar, text: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&Language::new(grammar.language)).ok()?;
    let budget = (text.len() / 1024 + 4) * PARSE_BUDGET;
    let mut calls = 0;
    let mut give_up = |_: &ParseState| {
        calls += 1;
        if calls > budget { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    };
    let bytes = text.as_bytes();
    let options = ParseOptions::new().progress_callback(&mut give_up);
    parser.parse_with_options(&mut |i, _| &bytes[i.min(bytes.len())..], None, Some(options))
}

/// A stretch of source that the cut never goes inside.
struct Atom<'a> {
    start: usize,
    end: usize,
    /// The node, if the atom is one whole node.
    node: Option<Node<'a>>,
    named: bool,
    leading: bool,
}

/// Source code, cut along its syntax tree.
///
/// A node that fits in a piece stays whole. One that does not is opened, and
/// its children take its place, down to nodes that fit; a leaf still too long
/// (a long string, comment or embedded script) is cut along blank lines and
/// lines. That gives a run of nodes in source order, which is packed into
/// pieces: a piece takes nodes until it reaches `SIZE.start`, then ends at the
/// next blank line if that comes before `SIZE.end`.
///
/// A cut falls only where a new line starts a named node. So a doc comment,
/// attribute or annotation on the lines directly above a node stays with it, a
/// closing `}` or `end` stays with what it closes, and `fn f() {` keeps the
/// signature with the start of its body. A node that fits is still opened when
/// the code on its first line makes the two too long together, as `fn f() {`
/// does with a body that fits alone. Code with no such place in a piece's
/// length, a long line or a long string of embedded code, is cut as text.
///
/// text-splitter's CodeSplitter cut between any two siblings, which put a
/// third of the doc comments in C# and Java in a different piece than their
/// method (bench/README.md, "Chunking").
fn code<'t>(grammar: &Grammar, text: &'t str, limit: Range<usize>) -> Vec<(usize, &'t str)> {
    let Some(tree) = parse(grammar, text) else {
        return plain_in(text, limit);
    };

    // Characters before each byte offset, since the limit counts characters.
    let mut before = Vec::with_capacity(text.len() + 1);
    let mut n = 0;
    for b in text.bytes() {
        before.push(n);
        n += usize::from(b & 0xC0 != 0x80);
    }
    before.push(n);
    let chars = |start: usize, end: usize| before[end] - before[start];
    let leads = |kind: &str| LEADING.iter().chain(grammar.leading).any(|k| kind.contains(k));

    // Text a node holds outside its children, such as a string's content in
    // some grammars, is kept as a span of its own. `closes` marks the last
    // child of its parent.
    enum Item<'a> {
        Node(Node<'a>, bool),
        Span(usize, usize),
    }
    let atoms = |open: &HashSet<usize>| {
        let mut found = Vec::new();
        let mut stack = vec![Item::Node(tree.root_node(), false)];
        while let Some(item) = stack.pop() {
            let (start, end, node, closes) = match item {
                Item::Node(n, closes) => (n.start_byte(), n.end_byte(), Some(n), closes),
                Item::Span(s, e) => (s, e, None, false),
            };
            if chars(start, end) < limit.end && node.is_none_or(|n| !open.contains(&n.id())) {
                found.push((start, end, node, closes));
                continue;
            }
            if let Some(n) = node.filter(|n| n.child_count() > 0) {
                let mut items = Vec::new();
                let mut at = start;
                let mut cursor = n.walk();
                for child in n.children(&mut cursor) {
                    if child.start_byte() > at {
                        items.push(Item::Span(at, child.start_byte()));
                    }
                    at = at.max(child.end_byte());
                    items.push(Item::Node(child, child.end_byte() == end));
                }
                if end > at {
                    items.push(Item::Span(at, end));
                }
                stack.extend(items.into_iter().rev());
                continue;
            }
            for (offset, piece) in plain_in(&text[start..end], limit.clone()) {
                found.push((start + offset, start + offset + piece.len(), None, false));
            }
        }
        let atoms: Vec<Atom> = found
            .into_iter()
            .filter_map(|(start, end, node, closes)| {
                let span = &text[start..end];
                let trimmed = span.trim();
                let start = start + (span.len() - span.trim_start().len());
                let own_line = text[..start].rsplit('\n').next().is_none_or(|l| l.trim().is_empty());
                let end = start + trimmed.len();
                // A line may start with an anonymous token that begins an item
                // (`, b`, `| Some x ->`, `.map(f)`, Go's `func`) or one that
                // closes the code above it (`}`, `end`, a `;;` alone on its line).
                let begins_item = !closes && !text[end..].split('\n').next().unwrap_or("").trim().is_empty();
                (!trimmed.is_empty()).then(|| Atom {
                    start,
                    end,
                    node,
                    named: node.is_none_or(|n| {
                        if n.is_named() { !TRAILING.iter().any(|t| n.kind().ends_with(t)) } else { begins_item }
                    }),
                    leading: own_line && node.is_some_and(|n| leads(n.kind())),
                })
            })
            .collect();
        atoms
    };
    // Runs of atoms with no place to cut between them, as ranges of atoms.
    let runs = |atoms: &[Atom]| {
        let mut runs: Vec<Range<usize>> = Vec::new();
        for (i, atom) in atoms.iter().enumerate() {
            match runs.last_mut() {
                Some(r) if !atom.named || !text[atoms[i - 1].end..atom.start].contains('\n') => r.end = i + 1,
                _ => runs.push(i..i + 1),
            }
        }
        runs
    };
    let span = |atoms: &[Atom], r: &Range<usize>| (atoms[r.start].start, atoms[r.end - 1].end);

    // Open the nodes that span lines inside a run too long for a piece, until
    // no such run has any left.
    let mut open = HashSet::new();
    let (atoms, runs) = loop {
        let atoms = atoms(&open);
        let runs = runs(&atoms);
        let mut more = false;
        for r in &runs {
            let (start, end) = span(&atoms, r);
            if chars(start, end) < limit.end {
                continue;
            }
            for atom in &atoms[r.clone()] {
                if let Some(n) = atom.node.filter(|n| n.child_count() > 0 && text[atom.start..atom.end].contains('\n')) {
                    more |= open.insert(n.id());
                }
            }
        }
        if !more {
            break (atoms, runs);
        }
    };

    // A leading node joins the line below it, unless that leaves the two too
    // long for a piece; then the node below stays whole instead.
    let mut groups: Vec<Range<usize>> = Vec::new();
    for r in runs {
        match groups.last_mut() {
            Some(g)
                if atoms[g.end - 1].leading
                    && text[atoms[g.end - 1].end..atoms[r.start].start].matches('\n').count() == 1
                    && chars(atoms[g.start].start, atoms[r.end - 1].end) < limit.end =>
            {
                g.end = r.end;
            }
            _ => groups.push(r),
        }
    }
    // A group still too long has no node boundary at a line start to cut at.
    // Cut it as text where it spans lines (a string of embedded code), else
    // between nodes (a long line).
    let mut parts: Vec<(usize, usize)> = Vec::new();
    for g in groups {
        let (start, end) = span(&atoms, &g);
        if chars(start, end) < limit.end {
            parts.push((start, end));
        } else if text[start..end].contains('\n') {
            parts.extend(plain_in(&text[start..end], limit.clone()).into_iter().map(|(o, p)| (start + o, start + o + p.len())));
        } else {
            let mut part = (start, atoms[g.start].end);
            for atom in &atoms[g.start + 1..g.end] {
                if chars(part.0, atom.end) < limit.end {
                    part.1 = atom.end;
                } else {
                    parts.push(part);
                    part = (atom.start, atom.end);
                }
            }
            parts.push(part);
        }
    }

    // A piece takes parts until it reaches limit.start. It then ends at the
    // next blank line, or at the end of the file, if that is in reach;
    // otherwise at the next part.
    let blank = |at: usize| text[text[..at].trim_end().len()..at].matches('\n').count() >= 2;
    let fits = |i: usize, k: usize| chars(parts[i].0, parts[k - 1].1) < limit.end;
    let mut pieces: Vec<(usize, &str)> = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let mut j = i + 1;
        while j < parts.len() && chars(parts[i].0, parts[j - 1].1) < limit.start && fits(i, j + 1) {
            j += 1;
        }
        if j < parts.len() && !blank(parts[j].0) {
            let mut k = j + 1;
            while k <= parts.len() && fits(i, k) {
                if k == parts.len() || blank(parts[k].0) {
                    j = k;
                    break;
                }
                k += 1;
            }
        }
        let (start, end) = (parts[i].0, parts[j - 1].1);
        pieces.push((start, &text[start..end]));
        i = j;
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn non_space(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// Every character outside whitespace lands in exactly one piece, in order,
    /// and no piece passes the upper bound unless one line does.
    fn check_cover<'t>(ext: &str, text: &'t str) -> Vec<(usize, &'t str)> {
        let pieces = offsets(ext, text);
        let mut at = 0;
        for (offset, piece) in &pieces {
            assert!(*offset >= at, "pieces overlap at byte {offset}");
            assert_eq!(&text[*offset..*offset + piece.len()], *piece);
            at = offset + piece.len();
            let longest_line = piece.lines().map(|l| l.chars().count()).max().unwrap_or(0);
            assert!(piece.chars().count() < SIZE.end || longest_line >= SIZE.end, "{} chars", piece.chars().count());
        }
        let joined: String = pieces.iter().map(|(_, p)| *p).collect();
        assert_eq!(non_space(&joined), non_space(text));
        pieces
    }

    #[test]
    fn every_grammar_loads() {
        for g in GRAMMARS {
            let mut parser = Parser::new();
            parser.set_language(&Language::new(g.language)).unwrap_or_else(|e| panic!("{:?}: {e}", g.extensions));
        }
    }

    /// Each grammar has a file `testdata/two.<ext>` of two definitions with a
    /// comment above each. The piece size fits either definition, and fits
    /// the first with the second's comment, but not both: only a comment kind
    /// that leads keeps the second comment out of the first piece.
    #[test]
    fn each_language_keeps_a_comment_with_the_code_below() {
        let n = |s: &str| s.chars().count();
        let mut failed = Vec::new();
        for g in GRAMMARS {
            let (ext, text) = g
                .extensions
                .iter()
                .find_map(|e| std::fs::read_to_string(format!("{}/src/chunk/testdata/two.{e}", env!("CARGO_MANIFEST_DIR"))).ok().map(|t| (e, t)))
                .unwrap_or_else(|| panic!("no testdata/two.* for {:?}", g.extensions));
            if parse(g, &text).is_none_or(|t| t.root_node().has_error()) {
                failed.push(format!("two.{ext} does not parse"));
                continue;
            }
            let second = text.find("Second.").unwrap();
            let cut = text[..second].rfind('\n').map_or(0, |i| i + 1);
            let comment_end = second + text[second..].find('\n').unwrap();
            let (one, two) = (text[..cut].trim(), text[cut..].trim());
            let end = (n(text[..comment_end].trim_start()) + 1).max(n(two) + 1);
            assert!(end < n(text.trim()), "two.{ext} is too small");
            let pieces: Vec<&str> = code(g, &text, end - 1..end).into_iter().map(|(_, p)| p).collect();
            if pieces != [one, two] {
                failed.push(format!("two.{ext}: {pieces:#?}"));
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    /// tree-sitter-objc takes exponential time on this file (issue #22).
    #[test]
    fn a_parse_that_runs_too_long_is_given_up() {
        let objc = grammar("m").unwrap();
        assert!(parse(objc, include_str!("chunk/testdata/budget.m")).is_none());
        assert!(parse(objc, include_str!("chunk/testdata/two.m")).is_some());
    }

    #[test]
    fn a_doc_comment_stays_with_its_function() {
        // Two functions of 1,800 characters, so the file does not fit one piece.
        let body = "    let x = 1;\n".repeat(120);
        let text = format!("/// Adds one.\n/// Really.\nfn one() {{\n{body}}}\n\n/// Adds two.\n#[inline]\nfn two() {{\n{body}}}\n");
        let pieces = check_cover("rs", &text);
        assert_eq!(pieces.len(), 2);
        assert!(pieces[1].1.starts_with("/// Adds two."));
    }

    #[test]
    fn a_long_function_keeps_its_signature_with_its_body() {
        let body = "    let x = compute(1, 2, 3);\n".repeat(150);
        let text = format!("// Before.\nfn small() {{}}\n\n/// Does a lot.\npub fn long(a: u32) -> u32 {{\n{body}    a\n}}\n");
        let pieces = check_cover("rs", &text);
        assert!(pieces.len() > 1);
        let at = pieces.iter().position(|(_, p)| p.contains("pub fn long")).unwrap();
        assert!(pieces[at].1.contains("let x = compute"), "signature alone: {:?}", pieces[at].1);
        assert!(pieces.iter().all(|(_, p)| !p.trim_start().starts_with('}')));
    }

    #[test]
    fn a_long_line_is_still_cut() {
        let text = format!("const A: [u32; 2000] = [{}];\n", "1, ".repeat(2000));
        let pieces = check_cover("rs", &text);
        assert!(pieces.len() > 1);
        assert!(pieces.iter().all(|(_, p)| p.chars().count() < SIZE.end));
    }

    #[test]
    fn text_outside_children_is_kept() {
        // A long string literal: one leaf over SIZE.end.
        let text = format!("fn f() {{\n    let s = \"{}\";\n}}\n", "word ".repeat(1000));
        check_cover("rs", &text);
    }

    /// Pieces of `text` in `ext`, after checking that the text parses.
    fn cut(ext: &str, text: &str, limit: Range<usize>) -> Vec<String> {
        let g = grammar(ext).unwrap();
        assert!(!parse(g, text).unwrap().root_node().has_error(), "{text}");
        code(g, text, limit).into_iter().map(|(_, p)| p.to_string()).collect()
    }

    #[test]
    fn a_piece_ends_at_a_blank_line_in_reach() {
        let text = "const A: u32 = 1;\nconst B: u32 = 2;\nconst C: u32 = 3;\n\n".repeat(20);
        let pieces = code(grammar("rs").unwrap(), &text, 80..400);
        assert!(pieces.len() > 1);
        for (offset, piece) in pieces {
            let rest = &text[offset + piece.len()..];
            assert!(rest.trim().is_empty() || rest.starts_with("\n\n"), "{piece:?}");
        }
    }

    #[test]
    fn a_body_that_fits_is_opened_when_its_signature_does_not_fit_with_it() {
        let body = "    if ready() {\n        go();\n    }\n".repeat(12);
        let block = format!("{{\n{body}}}");
        let text = format!("pub fn long(first: u32, second: u32, third: u32) {block}\n");
        let end = block.chars().count() + 1;
        let pieces = cut("rs", &text, end - 1..end);
        assert!(pieces.len() > 1);
        assert!(pieces[0].starts_with("pub fn long") && pieces[0].contains("go();"), "{:?}", pieces[0]);
        assert!(pieces[1..].iter().all(|p| p.starts_with("if ready() {")), "{pieces:#?}");
    }

    #[test]
    fn an_embedded_script_is_cut_at_lines() {
        // An interpolation mid-line joins each line to the next, so the
        // string is one run of nodes too long for a piece.
        let script = "    echo \"building ${name} step\" >> \"$out/log\"\n".repeat(150);
        let text = format!("{{\n  buildPhase = ''\n{script}  '';\n}}\n");
        let pieces = check_cover("nix", &text);
        assert!(pieces.len() > 1);
        for (offset, _) in pieces {
            assert!(text[..offset].trim_end_matches(' ').ends_with(['\n', '{']) || offset == 0, "cut mid-line at byte {offset}");
        }
    }

    #[test]
    fn a_go_func_keyword_starts_a_piece() {
        let short = "\tx := 1\n".repeat(200);
        let long = "\ty := 2\n".repeat(400);
        let text = format!("package p\n\nfunc one() {{\n{short}}}\nfunc two() {{\n{long}}}\n");
        let pieces = cut("go", &text, SIZE);
        assert!(pieces[0].ends_with("x := 1\n}"), "{:?}", &pieces[0][pieces[0].len() - 40..]);
        assert!(pieces[1].starts_with("func two() {"), "{:?}", &pieces[1][..40]);
    }

    #[test]
    fn a_leading_comma_starts_an_item() {
        let items: String = (0..60)
            .map(|i| format!("  {} Item\n      {{ name = \"item {i}\"\n      , size = {i}\n      }}\n", if i == 0 { '[' } else { ',' }))
            .collect();
        let text = format!("module M where\n\nitems :: [Item]\nitems =\n{items}  ]\n");
        let pieces = cut("hs", &text, SIZE);
        assert!(pieces.len() > 1);
        assert!(pieces[1..].iter().all(|p| p.starts_with(", Item")), "{pieces:#?}");
    }

    #[test]
    fn broken_code_still_covers() {
        let text = format!("fn f( {{\n{}\n", "    let x = ;\n".repeat(400));
        check_cover("rs", &text);
    }
}
