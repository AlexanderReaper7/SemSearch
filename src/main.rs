//! `semsearch`: feeds the hister instance and queries it.
//!
//! Every source lives in one instance of the hister fork, and a source is a
//! document type: `code`, `local` for files, `web`. See docs/decisions.md.

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use semsearch::{Source, ask, code, hister, history, time};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Local semantic search over code, files and web history")]
struct Cli {
    #[command(subcommand)]
    command: Command,
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
    /// content captured live by the extension survives a re-run. A run sends
    /// only entries visited since the last one, or everything when hister
    /// has lost what the last run sent.
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
        /// Only code under this directory, such as the checkout being worked
        /// in. Repeat for several. Its worktrees are other directories.
        #[arg(long = "in", value_name = "DIR")]
        under: Vec<PathBuf>,
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
    },
    /// Deep search: a local model searches the code, reads what it finds,
    /// and keeps only the places that answer, each with a line on why.
    ///
    /// The model is SEMSEARCH_LLM_MODEL at SEMSEARCH_LLM_URL, with
    /// SEMSEARCH_LLM_KEY as its bearer key. See docs/deep-search.md.
    Ask {
        /// JSON lines: a step per tool call, a hit per kept place, then done.
        #[arg(long)]
        json: bool,
        /// Only code under this directory for the searches and grep. Repeat
        /// for several.
        #[arg(long = "in", value_name = "DIR")]
        under: Vec<PathBuf>,
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
        Command::Ask { json, under, query } => {
            let result = deep_search(&query.join(" "), &under, json);
            if json && let Err(e) = &result {
                println!("{}", serde_json::to_string(&ask::Event::Error { message: format!("{e:#}") })?);
                std::process::exit(1);
            }
            result
        }
        Command::Search { source, limit, json, since, before, under, query } => {
            let since = since.as_deref().map(time::parse_bound).transpose()?;
            let before = before.as_deref().map(time::parse_bound).transpose()?;
            let source = match source {
                SourceArg::Web => Some(Source::Web),
                SourceArg::Files => Some(Source::Files),
                SourceArg::Code => Some(Source::Code),
                SourceArg::All => None,
            };
            let mut query = query.join(" ");
            if !under.is_empty() {
                if !matches!(source, Some(Source::Code)) {
                    anyhow::bail!("--in applies to code only, use --source code");
                }
                let under = under.iter().map(std::path::absolute).collect::<std::io::Result<Vec<_>>>()?;
                query = format!("{} {query}", code::under(&under));
            }
            let found = hister::search(source, &hister::Search { query: &query, since, before })?;
            for note in &found.notes {
                eprintln!("semsearch: {note}");
            }
            for hit in found.hits.into_iter().take(limit) {
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

fn deep_search(query: &str, under: &[PathBuf], json: bool) -> Result<()> {
    let under = under.iter().map(std::path::absolute).collect::<std::io::Result<Vec<_>>>()?;
    let corpus = ask::Corpus::indexed(&under)?;
    let mut search = ask::hister_search(&under);
    let home = std::env::var("HOME").unwrap_or_default();
    let mut print = |event: ask::Event| {
        if json {
            println!("{}", serde_json::to_string(&event).expect("an event serializes"));
            return;
        }
        match event {
            ask::Event::Step { tool, args, .. } => eprintln!("  {tool} {args}"),
            ask::Event::Hit { path, start, end, why, .. } => {
                let path = path.display().to_string();
                let path = path.strip_prefix(&home).map_or(path.clone(), |p| format!("~{p}"));
                println!("{path}:{start}-{end}\n  {why}");
            }
            ask::Event::Note { message } | ask::Event::Error { message } => eprintln!("semsearch: {message}"),
            ask::Event::Done { hits, turns, seconds, stopped } => {
                let stopped = stopped.map(|s| format!(", stopped by the {s} limit")).unwrap_or_default();
                eprintln!("{hits} hits, {turns} turns, {seconds:.1} s{stopped}");
            }
        }
    };
    ask::run(query, &corpus, ask::LIMITS, &mut ask::Llm::from_env(), &mut search, &mut print)
}
