//! Cuts a source file into pieces that hister embeds whole.
//!
//! hister passes text under `semantic_search.max_context_length` through as a
//! single chunk, so a piece cut here is exactly one vector there. Code splits
//! along the syntax tree, Markdown along headings, anything else along blank
//! lines.

use std::ops::Range;
use text_splitter::{ChunkConfig, CodeSplitter, MarkdownSplitter, TextSplitter};

/// Characters per piece. The upper bound stays under the instances'
/// max_context_length of 1024 (hister counts words, and code runs at roughly
/// 3-4 characters per word); the lower bound merges tiny items such as `use`
/// lines into their neighbours.
const SIZE: Range<usize> = 400..3000;

pub struct Piece {
    /// 1-based line of the first character.
    pub line: usize,
    /// 1-based column of the first character, in characters.
    pub column: usize,
    pub text: String,
}

fn language(ext: &str) -> Option<tree_sitter::Language> {
    Some(match ext {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "py" => tree_sitter_python::LANGUAGE.into(),
        "ts" | "mts" | "cts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "js" | "mjs" | "cjs" | "jsx" => tree_sitter_javascript::LANGUAGE.into(),
        "cs" => tree_sitter_c_sharp::LANGUAGE.into(),
        "java" => tree_sitter_java::LANGUAGE.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "nix" => tree_sitter_nix::LANGUAGE.into(),
        "sh" | "bash" => tree_sitter_bash::LANGUAGE.into(),
        _ => return None,
    })
}

pub fn split(ext: &str, text: &str) -> Vec<Piece> {
    let config = || ChunkConfig::new(SIZE);
    let offsets: Vec<(usize, &str)> = if let Some(lang) = language(ext) {
        match CodeSplitter::new(lang, config()) {
            Ok(s) => s.chunk_indices(text).collect(),
            Err(_) => TextSplitter::new(config()).chunk_indices(text).collect(),
        }
    } else if ext == "md" || ext == "markdown" {
        MarkdownSplitter::new(config()).chunk_indices(text).collect()
    } else {
        TextSplitter::new(config()).chunk_indices(text).collect()
    };

    let mut line = 1;
    let mut counted = 0;
    offsets
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
