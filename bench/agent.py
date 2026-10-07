# /// script
# requires-python = ">=3.12"
# ///
"""The deep search agent on the code search queries, against the seeded instance.
See bench/README.md.

    uv run bench/agent.py /srv/models/agent-candidates/Qwen3.5-4B-Q4_K_M.gguf --runs 5 --note "..."
    uv run bench/agent.py --endpoint http://host/v1 --model-name name ...   # a server already running

The agent gets the query and the instant results, then has read-only tools
(search, grep, read, git) and answers with the hits it keeps, one per line as
`path:start-end | why`. A hit is correct when its file is an answer's file and
its range holds the answer's line (found by anchor, as in run.py) and spans at
most MAX_SPAN lines.

Each model is served by its own llama-server on the card. That makes
InferMux's warden unload the reranker once; the first search loads it back.
Searches whose rerank failed are counted per run.

Writes bench/results/<time>-agent.json with every transcript and prints a summary.
"""

import argparse
import json
import os
import re
import shlex
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime
from pathlib import Path

import run

LLAMA = Path("/nix/store/0cw8p4njzwbzf2032fhcx9f9gf7r38sc-llama-cpp-0.5.0/bin")
PORT = 5099
CORPUS = run.PROJECTS
MAX_TURNS = 12  # model calls per query, the last one without tools
MAX_SPAN = 80  # lines a correct hit may span
OUTPUT_CHARS = 6000  # per tool result
READ_LINES = 150
INSTANT = 10  # instant results shown up front
GIT_COMMANDS = {"log", "show", "blame", "diff", "grep"}

SYSTEM = """You are a code search agent. The user asked a question about their code. Find the places in the code that answer it.

The code is a set of git repositories under one root: {repos}. Paths are relative to that root, e.g. `{example}`.

You are given the instant search results for the question. They are often right but may be wrong or incomplete. Use the tools to check them against the files and to search further:
- search: semantic search over the code, by meaning. Rephrase when results look wrong.
- grep: ripgrep regex over the files, for identifiers, strings and comments.
- read: read lines of a file.
- git: read-only git (log, show, blame, diff, grep) in one repository.

A correct place implements the behavior asked about, is a test that demonstrates it, or explicitly explains it in a comment or document. A mere mention or a call that delegates elsewhere is not.

Always start with a tool call: read the most promising instant result, or search again if none looks right. Try other wording and grep before deciding nothing answers the question; only then reply NONE.

When done, reply without a tool call. List only the places you checked and are confident about, best first, one per line. Show as many as are good, often one or two, never padding:
path:start-end | one short line on why it answers the question"""

TOOLS = [
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
        "name": "read", "description": f"Read lines of a file, numbered. At most {READ_LINES} lines per call.",
        "parameters": {"type": "object", "properties": {
            "path": {"type": "string"},
            "start": {"type": "integer", "description": "First line, default 1"},
            "end": {"type": "integer", "description": "Last line"}},
            "required": ["path"]}}},
    {"type": "function", "function": {
        "name": "git", "description": "Read-only git in one repository: log, show, blame, diff or grep.",
        "parameters": {"type": "object", "properties": {
            "repo": {"type": "string", "description": "Repository name, the first part of a path"},
            "args": {"type": "string", "description": "Arguments, e.g. `log --oneline -10 -- src/main.rs`"}},
            "required": ["repo", "args"]}}},
]

HIT = re.compile(r"^\s*(?:[-*\d.)]+\s*)?`?([^\s`|:]+):(\d+)(?:-(\d+))?`?\s*\|\s*(.+)$")
# Lenient on decoration, strict on content (proposed 2026-10-07): bullets, bold,
# backticks and a literal `path:` label are dropped; a file, a line and a why
# are still required, in that order.
DECORATION = re.compile(r"\*\*|`")
LABEL = re.compile(r"^\s*(?:[-*]|\d+[.)])?\s*(?:path\s*:\s*)?", re.I)
LENIENT = re.compile(r"^([\w.@+/-]+):(\d+)(?:-(\d+))?\s*\|\s*(.+)$")


# Tools


def env() -> dict:
    return os.environ | {"SEMSEARCH_URL": run.HISTER, "XDG_DATA_HOME": str(run.XDG)}


def relative(url: str) -> tuple[str, int]:
    """A vscode://file URL as a path under the corpus and its line."""
    path, line, _ = url.removeprefix("vscode://file").rsplit(":", 2)
    return str(Path(path).relative_to(CORPUS)), int(line)


def inside(path: str) -> Path | None:
    p = (CORPUS / path).resolve()
    return p if p == CORPUS or CORPUS in p.parents else None


def clip(text: str) -> str:
    return text if len(text) <= OUTPUT_CHARS else text[:OUTPUT_CHARS] + "\n[output cut]"


def semsearch(query: str, limit: int) -> tuple[list[dict], str | None]:
    p = subprocess.run(["semsearch", "search", "-n", str(limit), "--json", query], env=env(), capture_output=True, text=True)
    hits = [json.loads(l) for l in p.stdout.splitlines() if l.strip()]
    degraded = None if all(h.get("rerank_score") is not None for h in hits) else "no rerank score"
    return hits, degraded if p.returncode == 0 else p.stderr.strip()[-300:]


def show_hits(hits: list[dict]) -> str:
    out = []
    for i, h in enumerate(hits, 1):
        path, line = relative(h["url"])
        preview = "\n".join("    " + l for l in h.get("chunk", "").splitlines()[:8])
        out.append(f"{i}. {path}:{line}\n{preview}")
    return "\n".join(out) or "no results"


def call(name: str, args: dict, stats: dict) -> str:
    if name == "search":
        hits, degraded = semsearch(str(args.get("query", "")), min(int(args.get("limit") or 10), 20))
        if degraded:
            stats["degraded_searches"] += 1
        return show_hits(hits)
    if name == "grep":
        base = inside(str(args.get("path") or "."))
        if base is None:
            return "error: path outside the code root"
        cmd = ["rg", "-n", "--no-heading", "--max-count", "20", "--max-columns", "200", "-e", str(args["pattern"])]
        if args.get("glob"):
            cmd += ["-g", str(args["glob"])]
        p = subprocess.run(cmd + [str(base.relative_to(CORPUS)) if base != CORPUS else "."],
                           cwd=CORPUS, capture_output=True, text=True, timeout=30)
        return clip(p.stdout or p.stderr or "no matches")
    if name == "read":
        file = inside(str(args.get("path", "")))
        if file is None or not file.is_file():
            return "error: no such file"
        lines = file.read_text(errors="replace").splitlines()
        start = max(1, int(args.get("start") or 1))
        end = min(len(lines), int(args.get("end") or start + READ_LINES - 1), start + READ_LINES - 1)
        body = "\n".join(f"{n:5} {lines[n - 1]}" for n in range(start, end + 1))
        return clip(f"{args['path']} lines {start}-{end} of {len(lines)}\n{body}")
    if name == "git":
        repo = inside(str(args.get("repo", "")))
        argv = shlex.split(str(args.get("args", "")))
        if repo is None or not (repo / ".git").exists():
            return "error: no such repository"
        if not argv or argv[0] not in GIT_COMMANDS:
            return f"error: allowed commands are {', '.join(sorted(GIT_COMMANDS))}"
        p = subprocess.run(["git", "--no-pager", *argv], cwd=repo, capture_output=True, text=True, timeout=30)
        return clip(p.stdout or p.stderr or "no output")
    return f"error: no tool named {name}"


# The model


def chat(endpoint: str, model: str, messages: list, tools: list | None, sampling: dict) -> dict:
    body = {"model": model, "messages": messages, **sampling}
    if tools:
        body["tools"] = tools
    req = urllib.request.Request(f"{endpoint}/chat/completions", json.dumps(body).encode(), {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=600) as resp:
        return json.load(resp)


def new_stats() -> dict:
    return {"tool_calls": 0, "bad_calls": 0, "no_answer_turns": 0, "degraded_searches": 0,
            "prompt_tokens": 0, "completion_tokens": 0}


def opening(query: str, stats: dict) -> tuple[str, str]:
    """The system prompt and the first user message: the question and the instant results."""
    hits, degraded = semsearch(query, INSTANT)
    stats["degraded_searches"] += bool(degraded)
    repos = sorted(p.name for p in CORPUS.iterdir() if (p / ".git").exists())
    example = relative(hits[0]["url"])[0] if hits else f"{repos[0]}/README.md"
    return (SYSTEM.format(repos=", ".join(repos), example=example),
            f"Question: {query}\n\nInstant search results:\n{show_hits(hits)}")


def agent(endpoint: str, model: str, query: str, sampling: dict) -> dict:
    stats = new_stats()
    start = time.perf_counter()
    system, user = opening(query, stats)
    messages = [{"role": "system", "content": system}, {"role": "user", "content": user}]
    answer = None
    for turn in range(MAX_TURNS):
        last = turn == MAX_TURNS - 1
        if last:
            messages.append({"role": "user", "content": "No more tool calls. Answer now in the format asked for."})
        try:
            resp = chat(endpoint, model, messages, None if last else TOOLS, sampling)
        except Exception as e:  # a template or parser failure on the server
            stats["error"] = f"{type(e).__name__}: {e}"[:300]
            break
        usage = resp.get("usage") or {}
        stats["prompt_tokens"] += usage.get("prompt_tokens", 0)
        stats["completion_tokens"] += usage.get("completion_tokens", 0)
        msg = resp["choices"][0]["message"]
        calls = msg.get("tool_calls") or []
        kept = {k: v for k, v in msg.items() if k in ("role", "content", "tool_calls", "reasoning_content") and v}
        if kept.get("content") or calls:  # the server refuses an assistant message with neither
            messages.append(kept)
        if not calls:
            text = msg.get("content") or ""
            if parse_lenient(text) or text.strip().upper().startswith("NONE") or last:
                answer = text
                break
            stats["no_answer_turns"] += 1
            messages.append({"role": "user", "content": "Call a tool, or answer in the format asked for: path:start-end | why"})
            continue
        for c in calls:
            stats["tool_calls"] += 1
            name = c["function"]["name"]
            try:
                args = json.loads(c["function"].get("arguments") or "{}")
                out = call(name, args, stats)
            except Exception as e:
                stats["bad_calls"] += 1
                out = f"error: {type(e).__name__}: {e}"
            if out.startswith("error:"):
                stats["bad_calls"] += out.startswith("error: no tool") or out.startswith("error: allowed")
            messages.append({"role": "tool", "tool_call_id": c.get("id", name), "content": out})
    stats["seconds"] = round(time.perf_counter() - start, 2)
    stats["turns"] = sum(1 for m in messages if m["role"] == "assistant")
    return {"answer": answer, "stats": stats, "messages": messages}


def parse(text: str) -> list[dict]:
    """The strict reading: the format exactly as the prompt gives it."""
    out = []
    for line in text.splitlines():
        if m := HIT.match(line):
            start = int(m[2])
            out.append({"path": m[1], "start": start, "end": int(m[3] or start), "why": m[4].strip()})
    return out[:10]


def parse_lenient(text: str, starts: dict[str, list[int]] | None = None) -> list[dict]:
    """The lenient reading. A bare `path:N` where N is the first line of an
    indexed piece means that piece, as the instant results show pieces, so its
    span runs to the next piece's first line (run.Index.holding)."""
    out = []
    for line in text.splitlines():
        if not (m := LENIENT.match(LABEL.sub("", DECORATION.sub("", line)).strip())):
            continue
        start, end = int(m[2]), int(m[3] or m[2])
        if m[3] is None and starts and start in (s := starts.get(m[1], [])):
            i = s.index(start)
            end = s[i + 1] if i + 1 < len(s) else start + MAX_SPAN - 1
        out.append({"path": m[1], "start": start, "end": end, "why": m[4].strip()})
    return out[:10]


def piece_starts() -> dict[str, list[int]]:
    return {str(Path(f).relative_to(CORPUS)): sorted({line for line, _ in ps})
            for f, ps in run.Index().files.items() if Path(f).is_relative_to(CORPUS)}


def scores(row: dict, answers: list[tuple[str, int, str]], starts: dict[str, list[int]]) -> None:
    text = row["answer"] or ""
    row["hits"] = parse_lenient(text, starts)
    row["score"] = score(parse(text), answers)
    row["score_lenient"] = score(row["hits"], answers)


# Scoring


def answer_lines(q: dict) -> list[tuple[str, int, str]]:
    out = []
    for a in q["answers"]:
        file = CORPUS / a["repo"] / a["path"]
        if file.exists() and (line := run.locate(file, a)) is not None:
            out.append((f"{a['repo']}/{a['path']}", line, a["kind"]))
    return out


def score(hits: list[dict], answers: list[tuple[str, int, str]]) -> dict:
    marks = []
    for h in hits:
        lo, hi = min(h["start"], h["end"]), max(h["start"], h["end"])
        kinds = [k for path, line, k in answers if h["path"] == path and lo <= line <= hi and hi - lo < MAX_SPAN]
        marks.append(kinds[0] if kinds else None)
    first = next((i + 1 for i, k in enumerate(marks) if k), None)
    return {"shown": len(hits), "correct": sum(1 for k in marks if k), "first_correct": first,
            "found": first is not None, "found_code": "code" in marks, "marks": marks}


# Serving


def free_card() -> None:
    """InferMux's models off the card before the model's server loads. The
    warden would only unload them once the server shows GPU load, which a
    server that cannot allocate never does. The first search loads the
    reranker back, at its fresh size."""
    req = urllib.request.Request(f"{run.INFERMUX}/api/models/unload", b"", {"Authorization": f"Bearer {run.key()}"})
    urllib.request.urlopen(req, timeout=60).read()
    time.sleep(2)


def serve(model: Path, ctx: int, extra: list[str]) -> subprocess.Popen:
    with socket.socket() as s:
        if s.connect_ex(("127.0.0.1", PORT)) == 0:
            sys.exit(f"something already listens on :{PORT}")
    free_card()
    log = open(run.BENCH / "agent-llama.log", "a")
    proc = subprocess.Popen([str(LLAMA / "llama-server"), "--port", str(PORT), "-m", str(model), "--jinja",
                             "-ngl", "99", "-fa", "on", "--ctx-size", str(ctx), "--parallel", "1", *extra],
                            stdout=log, stderr=log)
    for _ in range(600):
        if proc.poll() is not None:
            sys.exit(f"llama-server exited with {proc.returncode}, see {run.BENCH / 'agent-llama.log'}")
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{PORT}/health", timeout=1)
            return proc
        except Exception:
            time.sleep(0.5)
    proc.kill()
    sys.exit("llama-server did not become healthy in 300 s")


def stop(proc: subprocess.Popen) -> None:
    proc.terminate()
    try:
        proc.wait(30)
    except subprocess.TimeoutExpired:
        proc.kill()


def warm_reranker() -> None:
    """A search until it comes back reranked: the warden unloaded the reranker
    when the model's server took the card, and the first rerank loads it."""
    for _ in range(30):
        if semsearch("warm-up", 3)[1] is None:
            return
        time.sleep(2)
    print("warning: searches still come back without rerank scores", file=sys.stderr)


def evaluate(one, name: str, qs: list[dict], runs: int, sampling: dict, parallel: int = 1) -> dict:
    """`one(query)` runs the agent once. Runs go `parallel` at a time; the
    local models take 1, since one llama-server serves one slot."""
    warm_reranker()
    starts = piece_starts()

    def go(job: tuple[dict, int]) -> dict:
        q, r = job
        res = one(q["text"]) | {"id": q["id"], "run": r}
        scores(res, answer_lines(q), starts)
        s, st = res["score_lenient"], res["stats"]
        print(f"  {q['id']:26} run {r}: found={'yes' if s['found'] else 'no ':3} shown={s['shown']} correct={s['correct']} "
              f"tools={st['tool_calls']} bad={st['bad_calls']} {st['seconds']:6.1f} s", file=sys.stderr)
        return res

    jobs = [(q, r) for q in qs for r in range(runs)]
    with ThreadPoolExecutor(parallel) as pool:
        rows = list(pool.map(go, jobs))
    return {"model": name, "sampling": sampling, "summary": summarize(rows), "rows": rows}


# Claude, through Claude Code


CLAUDE_TOOLS = [f"mcp__bench__{t['function']['name']}" for t in TOOLS]


def claude(argv: list[str], cwd: Path) -> list[dict]:
    """One `claude -p` call, as stream-json events. The environment loses this
    process's CLAUDE*/ANTHROPIC* variables, so a session started from inside
    Claude Code does not inherit its effort, messaging socket or session."""
    env = {k: v for k, v in os.environ.items() if not k.startswith(("CLAUDE", "ANTHROPIC"))}
    p = subprocess.run(["claude", "-p", *argv, "--output-format", "stream-json", "--verbose"],
                       cwd=cwd, env=env, capture_output=True, text=True, timeout=1800, stdin=subprocess.DEVNULL)
    events = [json.loads(l) for l in p.stdout.splitlines() if l.startswith("{")]
    if not any(e.get("type") == "result" for e in events):
        raise RuntimeError(f"claude exited {p.returncode}: {p.stderr.strip()[-500:]}")
    return events


def agent_claude(model: str, effort: str, query: str) -> dict:
    """The same agent as `agent`, with Claude Code as the loop: the harness's
    system prompt instead of Claude Code's, no built-in tools, only the four
    tools from mcp_tools.py, no settings, hooks, plugins, skills or CLAUDE.md
    from the user, in an empty directory. A session whose tool list is
    anything else stops the whole run."""
    stats = new_stats()
    start = time.perf_counter()
    system, user = opening(query, stats)
    with tempfile.TemporaryDirectory(prefix="bench-claude-", dir="/tmp") as tmp:
        cwd = Path(tmp) / "work"
        cwd.mkdir()
        stats_file = Path(tmp) / "stats.jsonl"
        mcp = Path(tmp) / "mcp.json"
        mcp.write_text(json.dumps({"mcpServers": {"bench": {
            "command": sys.executable, "args": [str(Path(__file__).with_name("mcp_tools.py"))],
            "env": {"BENCH_STATS": str(stats_file)}}}}))
        base = ["--model", model, "--effort", effort, "--setting-sources", "", "--disable-slash-commands",
                "--strict-mcp-config", "--system-prompt", system]
        events = claude([*base, "--tools", "", "--mcp-config", str(mcp), "--allowedTools", ",".join(CLAUDE_TOOLS),
                         "--max-turns", str(MAX_TURNS - 1), user], cwd)
        init = next(e for e in events if e.get("type") == "system" and e.get("subtype") == "init")
        if sorted(init.get("tools", [])) != sorted(CLAUDE_TOOLS):
            sys.exit(f"the Claude session had tools {init.get('tools')}, not only {CLAUDE_TOOLS}: stopping")
        result = next(e for e in events if e.get("type") == "result")
        answer = result.get("result") or ""
        if not (parse_lenient(answer) or answer.strip().upper().startswith("NONE")):
            # As in `agent`, without tools: out of turns, the last call; stopped
            # without the format, a reminder of it.
            if result.get("subtype") == "success":
                stats["no_answer_turns"] += 1
                nudge = "Answer in the format asked for: path:start-end | why"
            else:
                nudge = "No more tool calls. Answer now in the format asked for."
            more = claude([*base, "--tools", "", "--resume", result["session_id"], "--max-turns", "1", nudge], cwd)
            events += more
            result = next(e for e in more if e.get("type") == "result")
            answer = result.get("result") or ""
        shutil.rmtree(Path.home() / ".claude/projects" / str(cwd).replace("/", "-").replace(".", "-"), ignore_errors=True)
        for line in stats_file.read_text().splitlines() if stats_file.exists() else []:
            s = json.loads(line)
            stats["degraded_searches"] += s["degraded_searches"]
            stats["bad_calls"] += s["bad_calls"]
    messages = [{"role": "system", "content": system}, {"role": "user", "content": user}]
    for e in events:
        if e.get("type") in ("assistant", "user"):
            for block in e["message"]["content"] if isinstance(e["message"].get("content"), list) else []:
                if block.get("type") == "tool_use":
                    stats["tool_calls"] += 1
                messages.append({"role": e["type"], **block})
        if e.get("type") == "result":
            u = e.get("usage") or {}
            stats["prompt_tokens"] += u.get("input_tokens", 0) + u.get("cache_read_input_tokens", 0) + u.get("cache_creation_input_tokens", 0)
            stats["completion_tokens"] += u.get("output_tokens", 0)
            stats["cost_usd"] = round(stats.get("cost_usd", 0) + (e.get("total_cost_usd") or 0), 4)
    stats["model"] = init.get("model")
    stats["seconds"] = round(time.perf_counter() - start, 2)
    stats["turns"] = len({e["message"].get("id") for e in events if e.get("type") == "assistant"})
    return {"answer": answer or None, "stats": stats, "messages": messages}


def summarize(rows: list[dict]) -> dict:
    by_q: dict[str, list[dict]] = {}
    for r in rows:
        by_q.setdefault(r["id"], []).append(r)
    st = [r["stats"] for r in rows]

    def rates(key: str) -> dict:
        shown = sum(r[key]["shown"] for r in rows)
        return {
            "found_rate": round(sum(r[key]["found"] for r in rows) / len(rows), 3),
            "found_code_rate": round(sum(r[key]["found_code"] for r in rows) / len(rows), 3),
            "precision": round(sum(r[key]["correct"] for r in rows) / shown, 3) if shown else None,
            "mean_shown": round(shown / len(rows), 2),
            "per_query_found": {q: round(sum(r[key]["found"] for r in rs) / len(rs), 2) for q, rs in by_q.items()},
        }

    return {
        "runs": len(rows),
        "strict": rates("score"),
        "lenient": rates("score_lenient"),
        "no_answer_runs": sum(1 for r in rows if r["answer"] is None),
        "errors": sum(1 for s in st if "error" in s),
        "bad_calls": sum(s["bad_calls"] for s in st),
        "no_answer_turns": sum(s["no_answer_turns"] for s in st),
        "degraded_searches": sum(s["degraded_searches"] for s in st),
        "tool_calls_mean": round(statistics.mean(s["tool_calls"] for s in st), 2),
        "seconds": run.summarize([s["seconds"] for s in st]),
        "completion_tokens_mean": round(statistics.mean(s["completion_tokens"] for s in st)),
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("models", nargs="*", type=Path, help="GGUF files, each served by its own llama-server")
    ap.add_argument("--endpoint", help="an OpenAI-compatible base URL already serving, instead of GGUF files")
    ap.add_argument("--model-name", default="default")
    ap.add_argument("--claude", metavar="MODEL", help="run the agent as a Claude Code session with this model (eval only, never training data)")
    ap.add_argument("--effort", default="high", help="Claude Code's effort level, with --claude")
    ap.add_argument("--parallel", type=int, default=1, help="runs at a time, with --claude or --endpoint")
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--only", help="comma-separated query ids")
    ap.add_argument("--ctx", type=int, default=32768)
    ap.add_argument("--temperature", type=float, default=0.6)
    ap.add_argument("--think", action="store_true", help="turn the model's thinking on; cap it with --server-args \"--reasoning-budget N\"")
    ap.add_argument("--max-tokens", type=int, default=1024, help="per model call, thinking included")
    ap.add_argument("--server-args", default="", help="extra llama-server flags, e.g. MTP")
    ap.add_argument("--note", default="")
    ap.add_argument("--rescore", nargs="+", type=Path, help="score saved agent results again, in place")
    args = ap.parse_args()
    if args.rescore:
        rescore(args.rescore)
        return
    if not args.models and not args.endpoint and not args.claude:
        ap.error("give GGUF files, --endpoint or --claude")

    run.verify_checkout()
    qs = run.queries()
    if args.only:
        keep = set(args.only.split(","))
        qs = [q for q in qs if q["id"] in keep]
    sampling = {"temperature": args.temperature, "top_p": 0.95, "max_tokens": args.max_tokens,
                "chat_template_kwargs": {"enable_thinking": args.think}}

    result = {"kind": "agent", "when": datetime.now().isoformat(timespec="seconds"), "note": args.note,
              "setup": run.setup(), "max_turns": MAX_TURNS, "max_span": MAX_SPAN, "runs": args.runs,
              "queries": [q["id"] for q in qs], "server_args": args.server_args, "ctx": args.ctx, "models": []}
    if args.claude:
        # The Consumer Terms forbid training models on Claude's output; these
        # transcripts measure the ceiling and must stay out of training data.
        result["teacher_restricted"] = "Anthropic Consumer Terms: evaluation only, never training data"
    with run.instance():
        if args.claude:
            claude_sampling = {"effort": args.effort, "loop": "claude -p"}
            result["models"].append(evaluate(lambda q: agent_claude(args.claude, args.effort, q),
                                             f"claude:{args.claude}", qs, args.runs, claude_sampling, args.parallel))
        if args.endpoint:
            result["models"].append(evaluate(lambda q: agent(args.endpoint, args.model_name, q, sampling),
                                             args.model_name, qs, args.runs, sampling, args.parallel))
        for model in args.models:
            print(model.name, file=sys.stderr)
            proc = serve(model, args.ctx, shlex.split(args.server_args))
            endpoint = f"http://127.0.0.1:{PORT}/v1"
            try:
                result["models"].append(evaluate(lambda q: agent(endpoint, model.name, q, sampling),
                                                 model.name, qs, args.runs, sampling))
            finally:
                stop(proc)
    report(result)
    print(run.save("agent", result))


def report(result: dict) -> None:
    for m in result["models"]:
        s = m["summary"]
        st, le = s["strict"], s["lenient"]
        print(f"{m['model']:48} found {le['found_rate']:.2f} (strict {st['found_rate']:.2f}, code {le['found_code_rate']:.2f})  "
              f"precision {le['precision']}  shown {le['mean_shown']}  tools {s['tool_calls_mean']}  bad {s['bad_calls']}  "
              f"no-answer {s['no_answer_runs']}  errors {s['errors']}  degraded {s['degraded_searches']}  median {s['seconds']['median']} s")


def rescore(files: list[Path]) -> None:
    starts = piece_starts()
    answers = {q["id"]: answer_lines(q) for q in run.queries()}
    for f in files:
        result = json.loads(f.read_text())
        for m in result["models"]:
            for row in m["rows"]:
                scores(row, answers[row["id"]], starts)
            m["summary"] = summarize(m["rows"])
        result["rescored"] = datetime.now().isoformat(timespec="seconds")
        f.write_text(json.dumps(result, indent=1, ensure_ascii=False) + "\n")
        report(result)


if __name__ == "__main__":
    main()
