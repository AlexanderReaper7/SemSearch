//! `semsearch`: feeds the three hister instances and queries them.
//!
//! Each source is its own hister instance, because hister's semantic search
//! ignores `type:` and `label:` filters. See docs/decisions.md.

mod chunk;
mod hister;
mod history;
mod code;

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
    pub const ALL: [Source; 3] = [Source::Web, Source::Files, Source::Code];

    /// The hister instance that holds this source. Ports match prototype/*.yml.
    pub fn server(self) -> String {
        let (var, port) = match self {
            Source::Web => ("SEMSEARCH_WEB_URL", 4433),
            Source::Files => ("SEMSEARCH_FILES_URL", 4434),
            Source::Code => ("SEMSEARCH_CODE_URL", 4435),
        };
        std::env::var(var).unwrap_or_else(|_| format!("http://127.0.0.1:{port}"))
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
    /// Chunk every git repository under the given roots and sync it to the code instance.
    IndexCode {
        /// Directories that are repositories or contain repositories one level down.
        #[arg(required = true)]
        roots: Vec<PathBuf>,
    },
    /// Add Firefox history entries that the web instance lacks, as URL and title only.
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
        Command::Search { source, limit, json, query } => {
            let sources: Vec<Source> = match source {
                SourceArg::Web => vec![Source::Web],
                SourceArg::Files => vec![Source::Files],
                SourceArg::Code => vec![Source::Code],
                SourceArg::All => Source::ALL.to_vec(),
            };
            let hits = hister::search(&sources, &query.join(" "), limit)?;
            for hit in hits {
                if json {
                    println!("{}", serde_json::to_string(&hit)?);
                } else {
                    println!("{:.3}  [{}]  {}\n       {}", hit.similarity, hit.source, hit.url, hit.title);
                }
            }
            Ok(())
        }
    }
}
