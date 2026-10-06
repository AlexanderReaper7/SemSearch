//! `semsearch`: feeds the hister instance and queries it.
//!
//! Every source lives in one instance of the hister fork, and a source is a
//! document type: `code`, `local` for files, `web`. See docs/decisions.md.

mod chunk;
mod hister;
mod history;
mod code;
mod time;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Local semantic search over code, files and web history")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug)]
pub enum Source {
    Web,
    Files,
    Code,
}

impl Source {
    /// The query filter that selects this source.
    pub fn filter(self) -> &'static str {
        match self {
            Source::Web => "type:web",
            Source::Files => "type:file",
            Source::Code => "type:code",
        }
    }

    /// The source of a document, from hister's numeric type.
    pub fn of_type(t: Option<u64>) -> Option<Source> {
        match t? {
            0 => Some(Source::Web),
            1 | 2 => Some(Source::Files),
            3 => Some(Source::Code),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Source::Web => "web",
            Source::Files => "files",
            Source::Code => "code",
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Chunk every git repository under the given roots and sync it to hister.
    IndexCode {
        /// Directories that are repositories or contain repositories one level down.
        #[arg(required = true)]
        roots: Vec<PathBuf>,
    },
    /// Add Firefox history entries to hister, as URL, title and visit times.
    ///
    /// Never fetches a page and never overwrites a document that exists, so
    /// content captured live by the extension survives a re-run.
    ImportHistory {
        /// places.sqlite of a Firefox profile. Copied before reading, Firefox locks it.
        places: PathBuf,
    },
    /// Semantic search in one source, or in all of them with --source all.
    Search {
        #[arg(long, short, default_value = "code")]
        source: SourceArg,
        #[arg(long, short = 'n', default_value_t = 10)]
        limit: usize,
        /// JSON lines instead of text, for agents.
        #[arg(long)]
        json: bool,
        /// Only hits last visited or changed at or after this: `2026-09-22`,
        /// `2026-09-22 14:00`, or an age such as `3d` or `2w`.
        #[arg(long)]
        since: Option<String>,
        /// Only hits last visited or changed before this. Same forms as --since.
        #[arg(long)]
        before: Option<String>,
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SourceArg {
    Web,
    Files,
    Code,
    All,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::IndexCode { roots } => code::index(&roots),
        Command::ImportHistory { places } => history::import(&places),
        Command::Search { source, limit, json, since, before, query } => {
            let since = since.as_deref().map(time::parse_bound).transpose()?;
            let before = before.as_deref().map(time::parse_bound).transpose()?;
            let source = match source {
                SourceArg::Web => Some(Source::Web),
                SourceArg::Files => Some(Source::Files),
                SourceArg::Code => Some(Source::Code),
                SourceArg::All => None,
            };
            let query = query.join(" ");
            let hits = hister::search(source, &hister::Search { query: &query, since, before })?;
            for hit in hits.into_iter().take(limit) {
                if json {
                    println!("{}", serde_json::to_string(&hit)?);
                } else {
                    let visits = hit.visits.map(|n| format!("  ({n} visits)")).unwrap_or_default();
                    // Rerank score, then similarity. "-" is a hit the
                    // reranker did not see, "kw" a keyword hit.
                    let rerank = hit.rerank_score.map_or("  -  ".to_string(), |s| format!("{s:5.3}"));
                    let similarity = hit.similarity.map_or("  kw ".to_string(), |s| format!("{s:.3}"));
                    println!(
                        "{rerank} {similarity}  [{}]  {}  {}\n             {}{visits}",
                        hit.source,
                        time::short(hit.updated),
                        hit.url,
                        hit.title
                    );
                }
            }
            Ok(())
        }
    }
}
