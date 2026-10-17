//! Deep search: a small local model searches the code on the user's behalf,
//! checks what it finds, and keeps only the places that answer the question,
//! each with a line on why. See docs/deep-search.md.
//!
//! The prompt, the tools and the loop are bench/agent.py's, which the
//! 2026-10-07 eval measured, with paths relative to the home directory
//! instead of one corpus root.

use crate::{Source, code, hister};
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

/// How long a deep search may go on. At either limit the model gets one
/// more turn, without tools, to answer with what it has checked.
#[derive(Clone, Copy)]
pub struct Limits {
    /// Model calls, the last one without tools.
    pub turns: usize,
    pub time: Duration,
}

/// The user's choice, 2026-10-16.
pub const LIMITS: Limits = Limits { turns: 12, time: Duration::from_secs(60) };

/// How long the answer turn after a limit may take. It runs without
/// thinking, so on the GPU it takes a few seconds.
const ANSWER_TIME: Duration = Duration::from_secs(30);

const OUTPUT_CHARS: usize = 6000; // per tool result
const READ_LINES: usize = 150;
const INSTANT: usize = 10; // instant results shown up front
const SEARCH_MAX: usize = 20;
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);
const GIT_COMMANDS: [&str; 5] = ["blame", "diff", "grep", "log", "show"];

/// The git options the model may pass, per subcommand, matched as git
/// parses them. Everything else is refused, so an abbreviation git would
/// expand (`--open=` for `--open-files-in-pager`) or a short option hidden
/// in a group (`-iO cmd`) never reaches git.
struct GitOptions {
    /// Short options without a value, which may be grouped: `-in`.
    flags: &'static str,
    /// Short options with a value, attached or next: `-n5`, `-n 5`.
    valued: &'static str,
    /// Long options without a value.
    long: &'static [&'static str],
    /// Long options with a value, only as `--name=value`.
    long_valued: &'static [&'static str],
}

const LOG_LONG: &[&str] = &[
    "--abbrev-commit", "--all", "--cached", "--decorate", "--first-parent", "--follow", "--full-diff", "--graph",
    "--ignore-all-space", "--merges", "--name-only", "--name-status", "--no-color", "--no-decorate", "--no-merges",
    "--no-patch", "--numstat", "--oneline", "--patch", "--pretty", "--regexp-ignore-case", "--reverse", "--shortstat",
    "--staged", "--stat", "--word-diff", "--function-context", "--find-renames",
];
const LOG_LONG_VALUED: &[&str] = &[
    "--after", "--author", "--before", "--committer", "--date", "--diff-filter", "--format", "--grep", "--max-count",
    "--pretty", "--since", "--skip", "--unified", "--until", "--word-diff",
];

fn git_options(command: &str) -> GitOptions {
    match command {
        "grep" => GitOptions {
            flags: "EFGHIPchilnqvw",
            valued: "ABCem",
            long: &[
                "--all-match", "--and", "--break", "--cached", "--count", "--extended-regexp", "--files-with-matches",
                "--fixed-strings", "--heading", "--ignore-case", "--invert-match", "--line-number", "--not", "--or",
                "--perl-regexp", "--word-regexp",
            ],
            long_valued: &["--after-context", "--before-context", "--context", "--max-count"],
        },
        "blame" => GitOptions {
            flags: "CMeflnstw",
            valued: "L",
            long: &["--line-porcelain", "--porcelain", "--root", "--show-name", "--show-number"],
            long_valued: &["--date"],
        },
        // log, show and diff.
        _ => GitOptions { flags: "CMWbipsuw", valued: "GLSUn", long: LOG_LONG, long_valued: LOG_LONG_VALUED },
    }
}

/// Checks git arguments against the subcommand's options: the first one
/// refused, if any. A path may not leave the repository, since `git diff`
/// with a path outside the work tree reads it as `--no-index` would.
fn git_refusal(argv: &[String]) -> Option<String> {
    let opts = git_options(&argv[0]);
    let mut args = argv[1..].iter();
    let leaves = |a: &str| {
        let mut depth = 0i32;
        a.starts_with('/') || a.starts_with('~') || a.split('/').any(|c| {
            depth += match c { ".." => -1, "" | "." => 0, _ => 1 };
            depth < 0
        })
    };
    while let Some(a) = args.next() {
        if a == "--" {
            return args.find(|a| leaves(a)).map(|a| format!("{a} is outside the repository"));
        }
        if let Some(long) = a.strip_prefix("--") {
            let ok = match long.split_once('=') {
                Some((name, _)) => opts.long_valued.contains(&format!("--{name}").as_str()),
                None => opts.long.contains(&a.as_str()),
            };
            if !ok {
                return Some(format!("{a} is not allowed"));
            }
        } else if let Some(short) = a.strip_prefix('-').filter(|s| !s.is_empty()) {
            // -<n>: a commit count for log, context lines for grep.
            if short.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            for (i, c) in short.char_indices() {
                if opts.valued.contains(c) {
                    // The rest is its value, else the next argument is.
                    if i + c.len_utf8() == short.len() {
                        args.next();
                    }
                    break;
                }
                if !opts.flags.contains(c) {
                    return Some(format!("{a} is not allowed"));
                }
            }
        } else if leaves(a) {
            return Some(format!("{a} is outside the repository"));
        }
    }
    None
}

const SYSTEM: &str = "You are a code search agent. The user asked a question about their code. Find the places in the code that answer it.

The code is a set of git repositories under one root: {repos}. Paths are relative to that root, e.g. `{example}`.

You are given the instant search results for the question. They are often right but may be wrong or incomplete. Use the tools to check them against the files and to search further:
- search: semantic search over the code, by meaning. Rephrase when results look wrong.
- grep: ripgrep regex over the files, for identifiers, strings and comments.
- read: read lines of a file.
- git: read-only git (log, show, blame, diff, grep) in one repository.

A correct place implements the behavior asked about, is a test that demonstrates it, or explicitly explains it in a comment or document. A mere mention or a call that delegates elsewhere is not.

Always start with a tool call: read the most promising instant result, or search again if none looks right. Try other wording and grep before deciding nothing answers the question; only then reply NONE.

When done, reply without a tool call. List only the places you checked and are confident about, best first, one per line. Show as many as are good, often one or two, never padding:
path:start-end | one short line on why it answers the question";

fn tools() -> Value {
    json!([
        {"type": "function", "function": {
            "name": "search", "description": "Semantic search over the code. Returns the best pieces with path, line and a preview.",
            "parameters": {"type": "object", "properties": {
                "query": {"type": "string", "description": "What to look for, in words"},
                "limit": {"type": "integer", "description": "Number of results, default 10"}},
                "required": ["query"]}}},
        {"type": "function", "function": {
            "name": "grep", "description": "ripgrep a regex over the files. Returns matching lines as path:line:text.",
            "parameters": {"type": "object", "properties": {
                "pattern": {"type": "string", "description": "Rust regex"},
                "path": {"type": "string", "description": "File or directory to search in, default all repositories"},
                "glob": {"type": "string", "description": "Only files matching this glob, e.g. *.rs"}},
                "required": ["pattern"]}}},
        {"type": "function", "function": {
            "name": "read", "description": format!("Read lines of a file, numbered. At most {READ_LINES} lines per call."),
            "parameters": {"type": "object", "properties": {
                "path": {"type": "string"},
                "start": {"type": "integer", "description": "First line, default 1"},
                "end": {"type": "integer", "description": "Last line"}},
                "required": ["path"]}}},
        {"type": "function", "function": {
            "name": "git", "description": "Read-only git in one repository: log, show, blame, diff or grep.",
            "parameters": {"type": "object", "properties": {
                "repo": {"type": "string", "description": "Repository, as its path in the list, e.g. `Projects/SemSearch`"},
                "args": {"type": "string", "description": "Arguments, e.g. `log --oneline -10 -- src/main.rs`"}},
                "required": ["repo", "args"]}}},
    ])
}

/// What a deep search reports as it goes, in order: steps, then hits, then
/// done. `semsearch ask --json` prints each as a JSON line.
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Event {
    /// A tool call the model made. `turn` counts model calls from 1.
    Step { turn: usize, tool: String, args: Value },
    /// A place the model kept, best first.
    Hit { path: PathBuf, start: usize, end: usize, url: String, why: String },
    /// Something that went wrong without stopping the search.
    Note { message: String },
    /// `stopped` is `turns` or `time` when a limit made the model answer.
    Done {
        hits: usize,
        turns: usize,
        seconds: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        stopped: Option<&'static str>,
    },
    /// The search failed. Printed by the CLI, never sent by `run`.
    Error { message: String },
}

/// One model call: tools to offer, whether it thinks first, and how long it may take.
pub struct Call<'a> {
    pub tools: Option<&'a Value>,
    pub think: bool,
    pub timeout: Duration,
}

/// A chat model with tool calls.
pub trait Model {
    /// The assistant message of one completion, as the OpenAI API gives it,
    /// or None when it took longer than `call.timeout`.
    fn chat(&mut self, messages: &[Value], call: Call) -> Result<Option<Value>>;
}

/// An OpenAI-compatible endpoint, InferMux by default.
pub struct Llm {
    url: String,
    model: String,
    key: Option<String>,
    agent: ureq::Agent,
}

impl Llm {
    /// From `SEMSEARCH_LLM_URL` (the `/v1` base), `SEMSEARCH_LLM_MODEL` and
    /// `SEMSEARCH_LLM_KEY`, a bearer key if the endpoint wants one.
    pub fn from_env() -> Self {
        Llm {
            url: std::env::var("SEMSEARCH_LLM_URL").unwrap_or_else(|_| "http://127.0.0.1:5001/v1".to_string()),
            model: std::env::var("SEMSEARCH_LLM_MODEL").unwrap_or_else(|_| "Qwen3.5-4B-Q4_K_M".to_string()),
            key: std::env::var("SEMSEARCH_LLM_KEY").ok().filter(|k| !k.is_empty()),
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build()
                .into(),
        }
    }
}

impl Model for Llm {
    fn chat(&mut self, messages: &[Value], call: Call) -> Result<Option<Value>> {
        // The sampling of the eval's best run, 2026-10-07-035717. The
        // thinking budget is the server's (`--reasoning-budget`), so it
        // lives where the model is served.
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0.6,
            "top_p": 0.95,
            "max_tokens": 1280,
            "chat_template_kwargs": {"enable_thinking": call.think},
        });
        if let Some(tools) = call.tools {
            body["tools"] = tools.clone();
        }
        let url = format!("{}/chat/completions", self.url);
        let mut req = self.agent.post(&url).config().timeout_global(Some(call.timeout)).build();
        if let Some(key) = &self.key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }
        // llama-server stops generating when the client hangs up.
        let mut resp = match req.send_json(body) {
            Err(ureq::Error::Timeout(_)) => return Ok(None),
            r => r.with_context(|| format!("POST {url}"))?,
        };
        let status = resp.status();
        let mut reply: Value = match resp.body_mut().read_json() {
            Err(ureq::Error::Timeout(_)) => return Ok(None),
            r => r.context("chat response")?,
        };
        if !status.is_success() {
            bail!("{url} returned {status}: {reply}");
        }
        Ok(Some(reply["choices"][0]["message"].take()))
    }
}

/// A piece of code a search returned.
pub struct Piece {
    pub url: String,
    pub chunk: String,
}

/// Searches the code: the pieces, best first, and notes on what degraded.
pub type Search<'a> = dyn FnMut(&str, usize) -> Result<(Vec<Piece>, Vec<String>)> + 'a;

/// Instant search over the code in hister, under `dirs` if any are given.
pub fn hister_search(dirs: &[PathBuf]) -> impl FnMut(&str, usize) -> Result<(Vec<Piece>, Vec<String>)> + use<> {
    hister_search_at(hister::server(), dirs, TOOL_TIMEOUT)
}

/// `hister_search` on hister at `server`, each search given up after `timeout`.
fn hister_search_at(server: String, dirs: &[PathBuf], timeout: Duration) -> impl FnMut(&str, usize) -> Result<(Vec<Piece>, Vec<String>)> + use<> {
    let filter = (!dirs.is_empty()).then(|| code::under(dirs));
    move |q, limit| {
        let query = match &filter {
            Some(f) => format!("{f} {q}"),
            None => q.to_string(),
        };
        let found = hister::search_within(&server, Some(Source::Code), &hister::Search { query: &query, since: None, before: None }, Some(timeout))?;
        let pieces = found.hits.into_iter().take(limit).map(|h| Piece { url: h.url, chunk: h.chunk }).collect();
        Ok((pieces, found.notes))
    }
}

/// The code the model may see: files inside the indexed repositories, named
/// by their path relative to `root`.
pub struct Corpus {
    root: PathBuf,
    repos: Vec<PathBuf>,
    /// Where grep looks when the model names no path: the `--in`
    /// directories, or every repository.
    scope: Vec<PathBuf>,
}

impl Corpus {
    /// `repos` and `scope` are canonical paths, as `code::indexed_repositories` gives.
    pub fn new(root: PathBuf, repos: Vec<PathBuf>, scope: Vec<PathBuf>) -> Self {
        Corpus { root, repos, scope }
    }

    /// The home directory and every indexed repository, grep limited to `dirs`.
    pub fn indexed(dirs: &[PathBuf]) -> Result<Self> {
        let home = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?);
        let root = home.canonicalize().unwrap_or(home);
        let scope = dirs
            .iter()
            .map(|d| d.canonicalize().with_context(|| format!("{}", d.display())))
            .collect::<Result<Vec<_>>>()?;
        Ok(Corpus::new(root, code::indexed_repositories()?, scope))
    }

    fn rel(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).unwrap_or(path).display().to_string()
    }

    /// A path as the model wrote it, relative to the root, from `~`, or
    /// absolute, if it exists inside an indexed repository.
    fn resolve(&self, path: &str) -> Option<PathBuf> {
        let path = path.trim();
        let full = match path.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME").map(PathBuf::from)?.join(rest),
            None => self.root.join(path),
        };
        let real = full.canonicalize().ok()?;
        self.repos.iter().any(|r| real.starts_with(r)).then_some(real)
    }

    fn repo(&self, name: &str) -> Option<PathBuf> {
        let name = name.trim().trim_end_matches('/');
        self.resolve(name)
            .filter(|p| self.repos.contains(p))
            .or_else(|| self.repos.iter().find(|r| r.file_name().is_some_and(|n| n == name)).cloned())
    }
}

/// A `vscode://file` URL as its path and line.
fn piece_place(url: &str) -> Option<(PathBuf, usize)> {
    let rest = url.strip_prefix("vscode://file")?;
    let mut parts = rest.rsplitn(3, ':');
    let _column = parts.next()?;
    let line = parts.next()?.parse().ok()?;
    let path = parts.next()?.replace("%20", " ").replace("%25", "%");
    Some((PathBuf::from(path), line))
}

fn show_pieces(corpus: &Corpus, pieces: &[Piece]) -> String {
    let out: Vec<String> = pieces
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let place = match piece_place(&p.url) {
                Some((path, line)) => format!("{}:{line}", corpus.rel(&path)),
                None => p.url.clone(),
            };
            let preview: Vec<String> = p.chunk.lines().take(8).map(|l| format!("    {l}")).collect();
            format!("{}. {place}\n{}", i + 1, preview.join("\n"))
        })
        .collect();
    if out.is_empty() { "no results".to_string() } else { out.join("\n") }
}

fn clip(text: String) -> String {
    match text.char_indices().nth(OUTPUT_CHARS) {
        Some((cut, _)) => format!("{}\n[output cut]", &text[..cut]),
        None => text,
    }
}

/// Runs a read-only command for a tool: its output, else its errors, else "".
/// Killed after `timeout`.
fn run_tool(mut cmd: Command, timeout: Duration) -> String {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => return format!("error: {e}"),
    };
    // Read both pipes while it runs, so a full pipe cannot stall it.
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    if !matches!(child.wait_timeout(timeout), Ok(Some(_))) {
        let _ = child.kill();
        let _ = child.wait();
        return format!("error: no answer in {} s", timeout.as_secs_f64());
    }
    let (out, err) = (out.join().unwrap_or_default(), err.join().unwrap_or_default());
    String::from_utf8_lossy(if out.is_empty() { &err } else { &out }).into_owned()
}

fn int(args: &Value, key: &str) -> Option<usize> {
    args[key].as_u64().or_else(|| args[key].as_str()?.trim().parse().ok()).map(|n| n as usize)
}

fn call(corpus: &Corpus, search: &mut Search, notes: &mut Vec<String>, name: &str, args: &Value) -> String {
    let text = |key: &str| args[key].as_str().unwrap_or_default().to_string();
    match name {
        "search" => match search(&text("query"), int(args, "limit").unwrap_or(10).clamp(1, SEARCH_MAX)) {
            Ok((pieces, degraded)) => {
                notes.extend(degraded);
                show_pieces(corpus, &pieces)
            }
            Err(e) => format!("error: {e:#}"),
        },
        "grep" => {
            let targets = match args["path"].as_str().filter(|p| !p.trim().is_empty()) {
                Some(p) => match corpus.resolve(p) {
                    Some(path) => vec![path],
                    None => return "error: path outside the indexed code".to_string(),
                },
                None if corpus.scope.is_empty() => corpus.repos.clone(),
                None => corpus.scope.clone(),
            };
            let mut cmd = Command::new("rg");
            cmd.current_dir(&corpus.root)
                .args(["-n", "--no-heading", "--max-count", "20", "--max-columns", "200", "-e"])
                .arg(text("pattern"));
            if let Some(glob) = args["glob"].as_str().filter(|g| !g.is_empty()) {
                cmd.args(["-g", glob]);
            }
            cmd.arg("--");
            for t in &targets {
                cmd.arg(t.strip_prefix(&corpus.root).unwrap_or(t));
            }
            let out = run_tool(cmd, TOOL_TIMEOUT);
            clip(if out.is_empty() { "no matches".to_string() } else { out })
        }
        "read" => {
            let Some(file) = corpus.resolve(&text("path")).filter(|p| p.is_file()) else {
                return "error: no such file".to_string();
            };
            let bytes = match std::fs::read(&file) {
                Ok(bytes) => bytes,
                Err(e) => return format!("error: {e}"),
            };
            let content = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = content.lines().collect();
            let start = int(args, "start").unwrap_or(1).max(1);
            let end = int(args, "end").unwrap_or(start + READ_LINES - 1).min(start + READ_LINES - 1).min(lines.len());
            let body: Vec<String> = (start..=end).map(|n| format!("{n:5} {}", lines[n - 1])).collect();
            clip(format!("{} lines {start}-{end} of {}\n{}", text("path"), lines.len(), body.join("\n")))
        }
        "git" => {
            let Some(repo) = corpus.repo(&text("repo")) else {
                return "error: no such repository".to_string();
            };
            let Some(argv) = shlex::split(&text("args")) else {
                return "error: the arguments do not parse as shell words".to_string();
            };
            if argv.first().is_none_or(|c| !GIT_COMMANDS.contains(&c.as_str())) {
                return format!("error: allowed commands are {}", GIT_COMMANDS.join(", "));
            }
            if let Some(refusal) = git_refusal(&argv) {
                return format!("error: {refusal}");
            }
            let mut cmd = Command::new("git");
            cmd.current_dir(&repo)
                .env("GIT_PAGER", "cat")
                .env_remove("GIT_EXTERNAL_DIFF")
                .args(["--no-pager", "-c", "core.pager=cat"])
                .args(&argv);
            let out = run_tool(cmd, TOOL_TIMEOUT);
            clip(if out.is_empty() { "no output".to_string() } else { out })
        }
        _ => format!("error: no tool named {name}"),
    }
}

/// A place in the model's answer, as written.
#[derive(Debug, PartialEq)]
struct Answer {
    path: String,
    start: usize,
    end: usize,
    why: String,
}

/// The model's answer, read leniently, as bench/README.md's lenient score
/// does: bullets, bold, backticks and a `path:` label are dropped, and a bare
/// `path:N` is the line N. A path, a line and a why are still required.
fn parse(text: &str) -> Vec<Answer> {
    static DECORATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\*\*|`").unwrap());
    static LABEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*(?:[-*]|\d+[.)])?\s*(?:path\s*:\s*)?").unwrap());
    static HIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([\w.@+/~-]+):(\d+)(?:-(\d+))?\s*\|\s*(.+)$").unwrap());
    text.lines()
        .filter_map(|line| {
            let line = DECORATION.replace_all(line, "");
            let line = LABEL.replace(&line, "");
            let m = HIT.captures(line.trim())?;
            let start: usize = m[2].parse().ok()?;
            let end = m.get(3).and_then(|e| e.as_str().parse().ok()).unwrap_or(start).max(start);
            Some(Answer { path: m[1].to_string(), start, end, why: m[4].trim().to_string() })
        })
        .collect()
}

/// Keeps the fields a chat server accepts back, and only those with a value.
fn kept(msg: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in ["role", "content", "tool_calls", "reasoning_content"] {
        let v = &msg[key];
        let empty = v.is_null() || v.as_str() == Some("") || v.as_array().is_some_and(|a| a.is_empty());
        if !empty {
            out.insert(key.to_string(), v.clone());
        }
    }
    out.entry("role").or_insert(json!("assistant"));
    Value::Object(out)
}

/// One deep search for `query`. Reports each step, then each hit, then done.
pub fn run(query: &str, corpus: &Corpus, limits: Limits, model: &mut dyn Model, search: &mut Search, on: &mut dyn FnMut(Event)) -> Result<()> {
    let started = Instant::now();
    if corpus.repos.is_empty() {
        bail!("no code is indexed yet, run semsearch index-code first");
    }
    let (pieces, notes) = search(query, INSTANT)?;
    for message in notes {
        on(Event::Note { message });
    }
    let repos: Vec<String> = corpus.repos.iter().map(|r| corpus.rel(r)).collect();
    let example = pieces
        .first()
        .and_then(|p| piece_place(&p.url))
        .map(|(path, _)| corpus.rel(&path))
        .unwrap_or_else(|| format!("{}/README.md", repos[0]));
    let mut messages = vec![
        json!({"role": "system", "content": SYSTEM.replace("{repos}", &repos.join(", ")).replace("{example}", &example)}),
        json!({"role": "user", "content": format!("Question: {query}\n\nInstant search results:\n{}", show_pieces(corpus, &pieces))}),
    ];
    let tools = tools();
    let (mut answer, mut stopped, mut turns) = (String::new(), None, 0);
    let mut timed_out = false;
    for turn in 1..=limits.turns {
        let left = if timed_out { Duration::ZERO } else { limits.time.saturating_sub(started.elapsed()) };
        let last = turn == limits.turns || left.is_zero();
        if last {
            stopped = Some(if left.is_zero() { "time" } else { "turns" });
            messages.push(json!({"role": "user", "content": "No more tool calls. Answer now in the format asked for."}));
        }
        // The time limit holds inside a turn too: a call still thinking when
        // it runs out is dropped, and the answer turn comes next. That turn
        // thinks not at all, so it is short.
        let request = match last {
            true => Call { tools: None, think: false, timeout: ANSWER_TIME },
            false => Call { tools: Some(&tools), think: true, timeout: left },
        };
        let reply = model.chat(&messages, request)?;
        turns = turn;
        let Some(msg) = reply else {
            if last {
                break;
            }
            timed_out = true;
            continue;
        };
        let calls = msg["tool_calls"].as_array().cloned().unwrap_or_default();
        let content = msg["content"].as_str().unwrap_or_default().to_string();
        // A server refuses an assistant message with neither.
        if !content.is_empty() || !calls.is_empty() {
            messages.push(kept(&msg));
        }
        if calls.is_empty() {
            if last || !parse(&content).is_empty() || content.trim().to_uppercase().starts_with("NONE") {
                answer = content;
                break;
            }
            messages.push(json!({"role": "user", "content": "Call a tool, or answer in the format asked for: path:start-end | why"}));
            continue;
        }
        for c in &calls {
            let name = c["function"]["name"].as_str().unwrap_or_default().to_string();
            let raw = c["function"]["arguments"].as_str().unwrap_or("{}");
            let mut notes = Vec::new();
            // Calls left when the time runs out are not run, so at most one
            // tool's time (TOOL_TIMEOUT) goes past the limit.
            let out = match serde_json::from_str::<Value>(if raw.trim().is_empty() { "{}" } else { raw }) {
                _ if started.elapsed() >= limits.time => "error: out of time, not run".to_string(),
                Ok(args) => {
                    on(Event::Step { turn, tool: name.clone(), args: args.clone() });
                    call(corpus, search, &mut notes, &name, &args)
                }
                Err(e) => format!("error: the arguments are not JSON: {e}"),
            };
            for message in notes {
                on(Event::Note { message });
            }
            let id = c["id"].as_str().unwrap_or(&name).to_string();
            messages.push(json!({"role": "tool", "tool_call_id": id, "content": out}));
        }
    }

    let mut hits: Vec<(PathBuf, usize, usize)> = Vec::new();
    for a in parse(&answer) {
        let Some(path) = corpus.resolve(&a.path).filter(|p| p.is_file()) else {
            on(Event::Note { message: format!("left out {}: no such file in the indexed code", a.path) });
            continue;
        };
        if hits.contains(&(path.clone(), a.start, a.end)) {
            continue;
        }
        hits.push((path.clone(), a.start, a.end));
        let url = code::url(&path, a.start, 1);
        on(Event::Hit { path, start: a.start, end: a.end, url, why: a.why });
    }
    let seconds = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    on(Event::Done { hits: hits.len(), turns, seconds, stopped });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model that answers from a script and keeps what it was sent.
    /// A reply of `null` stands for a call that ran out of time.
    struct Scripted {
        replies: Vec<Value>,
        sent: Vec<(Vec<Value>, bool)>,
        calls: Vec<(bool, Duration)>,
    }

    fn scripted(replies: Vec<Value>) -> Scripted {
        Scripted { replies, sent: Vec::new(), calls: Vec::new() }
    }

    impl Model for Scripted {
        fn chat(&mut self, messages: &[Value], call: Call) -> Result<Option<Value>> {
            self.sent.push((messages.to_vec(), call.tools.is_some()));
            self.calls.push((call.think, call.timeout));
            let reply = self.replies.remove(0);
            Ok((!reply.is_null()).then_some(reply))
        }
    }

    fn tool_call(name: &str, args: Value) -> Value {
        json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": name, "arguments": args.to_string()}}]})
    }

    fn answer(text: &str) -> Value {
        json!({"role": "assistant", "content": text})
    }

    /// A root with one indexed repository, `proj`, holding `src/upload.rs`,
    /// and a file outside it.
    fn corpus(name: &str) -> (Corpus, PathBuf) {
        let root = std::env::temp_dir().join(format!("semsearch-ask-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("proj");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join("src/upload.rs"), "fn upload() {\n    retry(3);\n}\n").unwrap();
        std::fs::write(root.join("secret.txt"), "not code\n").unwrap();
        let root = root.canonicalize().unwrap();
        let repo = root.join("proj");
        (Corpus::new(root.clone(), vec![repo], Vec::new()), root)
    }

    fn instant(root: &Path) -> impl FnMut(&str, usize) -> Result<(Vec<Piece>, Vec<String>)> + use<> {
        let url = format!("vscode://file{}/proj/src/upload.rs:1:1", root.display());
        move |_, _| Ok((vec![Piece { url: url.clone(), chunk: "fn upload() {".to_string() }], Vec::new()))
    }

    #[test]
    fn a_checked_place_is_a_hit_with_its_why() {
        let (corpus, root) = corpus("hit");
        let mut model = scripted(vec![
                tool_call("read", json!({"path": "proj/src/upload.rs", "start": 1, "end": 3})),
                answer("proj/src/upload.rs:1-3 | retries the upload three times"),
        ]);
        let mut events = Vec::new();
        run("where are uploads retried", &corpus, LIMITS, &mut model, &mut instant(&root), &mut |e| events.push(e)).unwrap();

        let path = root.join("proj/src/upload.rs");
        assert_eq!(events[0], Event::Step { turn: 1, tool: "read".into(), args: json!({"path": "proj/src/upload.rs", "start": 1, "end": 3}) });
        assert_eq!(
            events[1],
            Event::Hit { path: path.clone(), start: 1, end: 3, url: code::url(&path, 1, 1), why: "retries the upload three times".into() }
        );
        assert!(matches!(events[2], Event::Done { hits: 1, turns: 2, stopped: None, .. }));
        assert_eq!(events.len(), 3);

        let (first, tools) = &model.sent[0];
        assert!(tools);
        assert!(first[1]["content"].as_str().unwrap().contains("1. proj/src/upload.rs:1\n    fn upload() {"));
        let (second, _) = &model.sent[1];
        let read = second.last().unwrap();
        assert_eq!(read["role"], "tool");
        assert!(read["content"].as_str().unwrap().contains("    2     retry(3);"));
    }

    #[test]
    fn out_of_time_the_model_answers_without_tools() {
        let (corpus, root) = corpus("time");
        let mut model = scripted(vec![answer("NONE")]);
        let mut events = Vec::new();
        let limits = Limits { turns: 12, time: Duration::ZERO };
        run("anything", &corpus, limits, &mut model, &mut instant(&root), &mut |e| events.push(e)).unwrap();

        let (sent, tools) = &model.sent[0];
        assert!(!tools);
        assert_eq!(sent.last().unwrap()["content"], "No more tool calls. Answer now in the format asked for.");
        assert!(matches!(events[..], [Event::Done { hits: 0, turns: 1, stopped: Some("time"), .. }]));
    }

    #[test]
    fn a_turn_that_runs_out_of_time_is_dropped_and_the_answer_turn_follows() {
        let (corpus, root) = corpus("in-turn");
        // The first call outlives the limit; the answer turn then answers.
        let mut model = scripted(vec![Value::Null, answer("proj/src/upload.rs:1-3 | retries the upload")]);
        let mut events = Vec::new();
        let limits = Limits { turns: 12, time: Duration::from_secs(5) };
        run("anything", &corpus, limits, &mut model, &mut instant(&root), &mut |e| events.push(e)).unwrap();

        let (think, timeout) = model.calls[0];
        assert!(think);
        assert!(timeout <= Duration::from_secs(5) && timeout > Duration::from_secs(4), "{timeout:?}");
        assert_eq!(model.calls[1], (false, ANSWER_TIME));
        assert!(!model.sent[1].1);
        assert_eq!(model.sent[1].0.last().unwrap()["content"], "No more tool calls. Answer now in the format asked for.");
        assert!(matches!(events.last().unwrap(), Event::Done { hits: 1, turns: 2, stopped: Some("time"), .. }));
    }

    #[test]
    fn tool_calls_left_when_the_time_runs_out_are_not_run() {
        let (corpus, root) = corpus("tools-time");
        let two = json!({"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "type": "function", "function": {"name": "search", "arguments": "{\"query\": \"a\"}"}},
            {"id": "c2", "type": "function", "function": {"name": "search", "arguments": "{\"query\": \"b\"}"}}]});
        let mut model = scripted(vec![two, answer("NONE")]);
        let mut events = Vec::new();
        let limits = Limits { turns: 12, time: Duration::from_millis(200) };
        let mut searches = 0;
        let mut first = instant(&root);
        // The instant search is quick; each search the model asks for takes
        // longer than the whole limit.
        let mut search = |q: &str, n: usize| {
            searches += 1;
            if searches > 1 {
                std::thread::sleep(Duration::from_millis(300));
            }
            first(q, n)
        };
        run("anything", &corpus, limits, &mut model, &mut search, &mut |e| events.push(e)).unwrap();

        assert_eq!(searches, 2);
        let results: Vec<&Value> = model.sent[1].0.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(results[1]["content"], "error: out of time, not run");
        assert!(!model.sent[1].1);
        assert!(matches!(events.last().unwrap(), Event::Done { turns: 2, stopped: Some("time"), .. }));
    }

    #[test]
    fn at_the_last_turn_the_model_answers_without_tools() {
        let (corpus, root) = corpus("turns");
        let mut model = scripted(vec![tool_call("grep", json!({"pattern": "retry"})), answer("proj/src/upload.rs:2 | calls retry")]);
        let mut events = Vec::new();
        let limits = Limits { turns: 2, time: Duration::from_secs(60) };
        run("anything", &corpus, limits, &mut model, &mut instant(&root), &mut |e| events.push(e)).unwrap();

        assert!(model.sent[0].1);
        assert!(!model.sent[1].1);
        let grep = model.sent[1].0.iter().find(|m| m["role"] == "tool").unwrap();
        assert!(grep["content"].as_str().unwrap().contains("proj/src/upload.rs:2:    retry(3);"), "{grep}");
        assert!(matches!(events.last().unwrap(), Event::Done { hits: 1, turns: 2, stopped: Some("turns"), .. }));
    }

    #[test]
    fn nothing_outside_the_indexed_repositories_is_read_or_shown() {
        let (corpus, root) = corpus("outside");
        let outside = root.join("secret.txt").display().to_string();
        let mut model = scripted(vec![
                tool_call("read", json!({"path": "secret.txt"})),
                tool_call("read", json!({"path": outside})),
                tool_call("read", json!({"path": "proj/../secret.txt"})),
                tool_call("grep", json!({"pattern": "code", "path": "secret.txt"})),
                answer("secret.txt:1 | holds a secret\nproj/missing.rs:1 | made up"),
        ]);
        let mut events = Vec::new();
        run("anything", &corpus, LIMITS, &mut model, &mut instant(&root), &mut |e| events.push(e)).unwrap();

        let tool_outputs: Vec<String> =
            model.sent.last().unwrap().0.iter().filter(|m| m["role"] == "tool").map(|m| m["content"].as_str().unwrap().to_string()).collect();
        assert_eq!(tool_outputs, ["error: no such file", "error: no such file", "error: no such file", "error: path outside the indexed code"]);
        assert!(!events.iter().any(|e| matches!(e, Event::Hit { .. })));
        assert!(events.contains(&Event::Note { message: "left out secret.txt: no such file in the indexed code".into() }));
        assert!(events.contains(&Event::Note { message: "left out proj/missing.rs: no such file in the indexed code".into() }));
    }

    #[test]
    fn git_refuses_what_writes_or_runs() {
        let (corpus, root) = corpus("git");
        let mut search = instant(&root);
        let mut notes = Vec::new();
        let mut git = |args: &str| call(&corpus, &mut search, &mut notes, "git", &json!({"repo": "proj", "args": args}));
        assert_eq!(git("diff --output=/tmp/x"), "error: --output=/tmp/x is not allowed");
        assert_eq!(git("diff --ext-diff"), "error: --ext-diff is not allowed");
        assert_eq!(git("log --textconv -p"), "error: --textconv is not allowed");
        // The pager command, spelled as git would accept it: grouped behind
        // other short options, attached, and as an abbreviation.
        assert_eq!(git("grep -Ovim foo"), "error: -Ovim is not allowed");
        assert_eq!(git("grep -iO'touch marker' retry"), "error: -iOtouch marker is not allowed");
        assert_eq!(git("grep -inO 'touch marker' retry"), "error: -inO is not allowed");
        assert_eq!(git("grep --open='touch marker' retry"), "error: --open=touch marker is not allowed");
        assert_eq!(git("grep --open-files-in-pager retry"), "error: --open-files-in-pager is not allowed");
        // Git expands any unique prefix, so only exact names pass, even of allowed options.
        assert_eq!(git("grep --cont=3 retry"), "error: --cont=3 is not allowed");
        assert_eq!(git("grep --line retry"), "error: --line is not allowed");
        assert_eq!(git("grep -f ../secret.txt"), "error: -f is not allowed");
        assert_eq!(git("blame --contents=../secret.txt src/upload.rs"), "error: --contents=../secret.txt is not allowed");
        // Files outside the repository, explicitly or as diff's implied --no-index.
        assert_eq!(git("diff --no-index ../secret.txt src/upload.rs"), "error: --no-index is not allowed");
        assert_eq!(git("diff ../secret.txt src/upload.rs"), "error: ../secret.txt is outside the repository");
        assert_eq!(git("diff -- src/../../secret.txt"), "error: src/../../secret.txt is outside the repository");
        let abs = root.join("secret.txt").display().to_string();
        assert_eq!(git(&format!("diff {abs} src/upload.rs")), format!("error: {abs} is outside the repository"));
        // What a search needs still runs: grouped flags, values, counts, ranges.
        let ok = |out: String| assert!(!out.starts_with("error: ") || out.contains("not a git repository"), "{out}");
        ok(git("grep -in -e retry"));
        ok(git("grep -A3 -n retry -- src"));
        ok(git("log -3 --oneline --author=me -- src/upload.rs"));
        ok(git("log -n 5 -Sretry -p"));
        ok(git("blame -L1,3 -w src/upload.rs"));
        ok(git("diff HEAD~1..HEAD --stat"));
        assert_eq!(git("push origin main"), "error: allowed commands are blame, diff, grep, log, show");
        assert_eq!(call(&corpus, &mut search, &mut notes, "git", &json!({"repo": "nope", "args": "log"})), "error: no such repository");
    }

    #[test]
    fn answers_are_read_leniently() {
        let read = |t: &str| parse(t).into_iter().map(|a| (a.path, a.start, a.end, a.why)).collect::<Vec<_>>();
        assert_eq!(read("a/b.rs:10-20 | why"), [("a/b.rs".into(), 10, 20, "why".into())]);
        assert_eq!(read("- **`a/b.rs:10-20`** | why"), [("a/b.rs".into(), 10, 20, "why".into())]);
        assert_eq!(read("2. path: a/b.rs:7 | why"), [("a/b.rs".into(), 7, 7, "why".into())]);
        assert_eq!(read("a/b.rs:20-10 | why"), [("a/b.rs".into(), 20, 20, "why".into())]);
        assert!(read("NONE").is_empty());
        assert!(read("a/b.rs:10-20").is_empty());
        assert!(read("Here is what I found:").is_empty());
    }

    #[test]
    fn a_turn_gets_only_the_time_left() {
        let (corpus, root) = corpus("time-left");
        let mut model = scripted(vec![answer("NONE")]);
        let mut found = instant(&root);
        // The instant search takes 300 ms of the second.
        let mut slow = |q: &str, n: usize| {
            std::thread::sleep(Duration::from_millis(300));
            found(q, n)
        };
        let limits = Limits { turns: 12, time: Duration::from_secs(1) };
        run("anything", &corpus, limits, &mut model, &mut slow, &mut |_| ()).unwrap();

        let (think, timeout) = model.calls[0];
        assert!(think);
        assert!(timeout <= Duration::from_millis(700), "{timeout:?}");
    }

    #[test]
    fn an_answer_turn_that_runs_out_of_time_ends_the_run() {
        let (corpus, root) = corpus("answer-time");
        // Both the tool turn and the answer turn outlive their time.
        let mut model = scripted(vec![Value::Null, Value::Null, answer("proj/src/upload.rs:1-3 | late")]);
        let mut events = Vec::new();
        let limits = Limits { turns: 12, time: Duration::from_secs(5) };
        run("anything", &corpus, limits, &mut model, &mut instant(&root), &mut |e| events.push(e)).unwrap();

        assert_eq!(model.calls.len(), 2);
        assert!(matches!(events[..], [Event::Done { hits: 0, turns: 2, stopped: Some("time"), .. }]));
    }

    #[test]
    fn a_search_hister_does_not_answer_is_given_up() {
        // Accepts and never answers.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let hold = std::thread::spawn(move || {
            let conn = listener.accept();
            std::thread::sleep(Duration::from_secs(5));
            drop(conn);
        });
        let mut search = hister_search_at(server, &[], Duration::from_millis(300));
        let started = Instant::now();
        assert!(search("anything", 5).is_err());
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        drop(hold);
    }

    #[test]
    fn a_tool_that_does_not_finish_is_killed() {
        let mut cmd = Command::new("sleep");
        cmd.arg("10");
        let started = Instant::now();
        assert_eq!(run_tool(cmd, Duration::from_millis(300)), "error: no answer in 0.3 s");
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    }

    #[test]
    fn tools_searches_and_the_answer_get_the_documented_time() {
        // docs/deep-search.md: a tool or search past 30 s is given up, and
        // the answer turn gets 30 s, so a run ends by 120 s at worst.
        assert_eq!(TOOL_TIMEOUT, Duration::from_secs(30));
        assert_eq!(ANSWER_TIME, Duration::from_secs(30));
    }
}
