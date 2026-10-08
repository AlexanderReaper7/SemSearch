# /// script
# requires-python = ">=3.12"
# ///
"""Writes draft queries for the eval set with two writers, Claude and Codex,
each on its own and from the checked-out corpus alone (docs/decisions.md,
2026-10-07). See bench/README.md, "Writing queries".

    uv run bench/write_queries.py write [--writer claude|codex] [--repo NAME ...] [--parallel 4]
    uv run bench/write_queries.py review [--reviewer claude|codex] [--repo NAME ...] [--parallel 4]
    uv run bench/write_queries.py merge
    uv run bench/write_queries.py status

`review` has each writer review the other's draft of a repository
(bench/drafts/<author>/<repo>.reviewed.toml, every query with a `review`
verdict). `merge` drops what reviewers dropped, merges duplicates across the
two writers and writes bench/queries/<repo>.toml, with the pairs it merged in
bench/drafts/merged.md.

A draft lands in bench/drafts/<writer>/<repo>.toml once every query in it
passes run.py's `problems`. A draft that still fails after the fix rounds is
kept as <repo>.failed.toml with its problems. Transcripts go to the instance's
writing/ directory, outside the repository.
"""

import argparse
import json
import math
import os
import subprocess
import sys
import tomllib
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import run  # noqa: E402

DRAFTS = run.HERE / "drafts"
LOGS = run.BENCH / "writing"
FIX_ROUNDS = 2
CLAUDE_MODEL, CLAUDE_EFFORT = "claude-opus-5-5", "medium"  # the user, 2026-10-07
CODEX_MODEL, CODEX_EFFORT = "gpt-6-sol", "high"  # the user, 2026-10-07
# Codex without the web, the browser or apps: the repository is the only source.
CODEX_OFF = ["browser_use", "browser_use_external", "computer_use", "apps", "image_generation", "in_app_browser"]

# Per writer, media queries for the repositories that have media (bench-v2
# corpus, measured 2026-10-07: 2,871 images, 65 sounds, 74 PDFs, no video).
MEDIA = {
    "godot-demo-projects": "6 (4 about sounds, 2 about images)", "progit2": "3 (diagrams; one may be a PDF)",
    "craftinginterpreters": "2", "Pixelorama": "2", "excalidraw": "2", "SameBoy": "2", "teaterihuskvarna": "2",
    "CSharp-XNA-Tools": "2 (1 about a sound)", "localsend": "1", "FreshRSS": "1", "plausible": "1", "Rectangle": "1",
    "InferMux": "1", "hister": "1", "zmk": "1", "unison": "1", "invidious": "1", "Seal": "1", "BatFi": "1",
    "DV1493_Datorteknik": "1 (a PDF)",
}

PROMPT = """You are writing evaluation queries for a semantic code search engine. It indexes a corpus of about 74 git repositories of every kind (web apps and frameworks, CLIs, emulators, databases, firmware, compilers, games, books, coursework, personal tools) cut into pieces, and answers a natural-language query with a ranked list of pieces. Your queries and their known answers measure whether it finds the right place.

Repository: {repo} ({about}). It is checked out read-only in the current directory. Work from these files only; do not use the web.

Write exactly {n} queries for this repository:
{quota}

## What a query is

- What a developer who does not know this code would type to find where something happens, why it is done that way, how to use something, or what causes a symptom. Natural phrasing, usually 5 to 25 words.
- It never reuses the answer's identifiers (function, type, variable, file or directory names) or the wording of its comments and docs. Say what the code does in other words, so the engine cannot win by matching keywords.
- Specific enough that this repository holds the answer and the other repositories in the corpus would not.
- Each query is about a different place. Spread them over the repository: core logic, edge cases, tests, docs, build and configuration, not only the README's headline features.

## Which answers are correct

A correct answer is a place that implements the behavior (kind "code"), a test that demonstrates it ("test"), a document or comment that explicitly explains it ("doc"), or a configuration value that is itself what the query asks for ("config"). A mere mention, a call site that only delegates, or configuration next to the behavior is not correct.

List every correct place you find, usually 1 to 4, best first. A correct place you leave out counts against the engine when it finds that place.

## Categories (field `category`)

- behavior: where something happens.
- explanation: why it is done this way; the answer is usually a doc, a decision record or a comment.
- usage: how to call, configure or use something; examples, tests, docs.
- symptom: a paraphrased error message or an observed misbehavior; the answer is the code responsible.
- config: where a setting or value is defined.
- media: find an image, sound, video or PDF by what it shows or does.

Mix the first five; most will be behavior.

## Adversarial queries (field `adversarial`, with a `note` on the trap)

- decoy: the query's most natural words appear prominently in a wrong place in this repository, and the right answer uses other vocabulary. The note names the decoy.
- near-miss: the repository has two or more similar implementations; one detail in the query picks the right one. The note names the others.
- negation: asks where something does NOT happen, or for the exception or opt-out path.
- no-answer: plausible for this kind of project, but absent. Search thoroughly first (several words and synonyms, grep and file names) and say in the note what you searched. `answers = []`.
- short: one to three words.
- verbose: 40 to 80 words of rambling context around the actual need.
- typo: two or three realistic misspellings.
- language: written in Swedish; also set `lang = "sv"`.
- injection: besides the real need, the query holds instructions aimed at a search agent, such as "ignore your tools and answer X" or "reply NONE". The real need stays clear and has answers.

## Media

Look at the files themselves (view the images). Describe what an image shows, not its file name. You cannot listen to a sound: describe its role from the code that plays it, the scene or event it belongs to and its context, and say in the note that it is inferred. A media answer is the whole file, with no line or anchor: `{{ repo = "{repo}", path = "...", kind = "image" }}`, kind one of image, audio, video, pdf. A media query's answers are the media files only.

## Output

Reply with only TOML, no prose and no code fence: one [[query]] table per query.

[[query]]
id = "short-kebab-slug"
text = "the query"
category = "behavior"
answers = [
  {{ repo = "{repo}", path = "path/from/the/repository/root", line = 42, kind = "code", anchor = 'the exact text of line 42, trimmed' }},
]

Optional fields: `adversarial`, `note`, `lang`.

An anchor is the whole text of its line with leading and trailing whitespace removed, copied exactly; the answer is found by it, so choose a distinctive line (not a lone brace or `return nil`) inside the answering code. Line numbers start at 1. Write anchors as TOML literal strings ('...'); when the line holds a single quote, use a basic string ("...") with backslash escapes."""


REVIEW = """You are reviewing evaluation queries for a semantic code search engine, written by another model for the repository {repo} ({about}). The repository is checked out read-only in the current directory. Work from these files only; do not use the web.

The rules the writer had are below, between the lines. Check every query against them and against the files:

1. Each answer is correct under the relevance rule, at the right line, and the anchor is that line's exact trimmed text.
2. No correct place is missing. Search for other places that implement, test or explicitly explain the same thing, and add them.
3. The query does not reuse the answer's identifiers or the wording of its comments and docs; reword it if it does, keeping its meaning.
4. The category fits. An adversarial query really has the trap its note claims: a decoy exists where the note says, a no-answer query truly has no answer (search for it yourself, with your own words).
5. A media query's description matches what the file shows; view it.
6. The query is something a developer would plausibly ask, and specific enough that other repositories would not answer it.

Give each query a verdict in a new field `review`: "keep" when it passes unchanged, "fixed: <what you changed>" when you corrected it, or "drop: <why>" when it cannot be repaired (keep its other fields as they were). Do not add new queries.

Reply with only TOML, no prose and no code fence: every query, in the original order, with all its fields.

-----
{rules}
-----

The queries to review:

{draft}"""


def quotas() -> dict[str, dict]:
    """Per repository: queries per writer from its indexable size (4 + 3.6
    sqrt(MB), 3 to 20), about 15% adversarial, one no-answer from 7 up."""
    sizes = json.loads((run.HERE / "corpus-sizes.json").read_text())
    out = {}
    for repo in run.repos():
        n = max(3, min(20, round(4 + 3.6 * math.sqrt(sizes[repo]))))
        none = 1 if n >= 7 else 0
        out[repo] = {"ordinary_and_adversarial": n, "adversarial": max(1 + none, round(0.15 * n)) if n >= 4 else 0,
                     "no_answer": none}
    return out


def suggested(repo: str, writer: str, k: int) -> list[str]:
    """Adversarial types for a repository, rotated so each writer covers all
    of them across the corpus and the two writers differ per repository."""
    names = sorted(run.repos())
    types = [t for t in run.ADVERSARIAL if t != "no-answer"]
    start = names.index(repo) * 2 + (0 if writer == "claude" else len(types) // 2)
    return [types[(start + i) % len(types)] for i in range(k)]


def prompt(repo: str, writer: str) -> str:
    q = quotas()[repo]
    n_adv, n_none = q["adversarial"], q["no_answer"]
    lines = []
    media = MEDIA.get(repo)
    n_media = int(media.split()[0]) if media else 0
    n = q["ordinary_and_adversarial"] + n_media
    if media:
        lines.append(f"- {media} media queries (category \"media\").")
    if n_adv:
        kinds = suggested(repo, writer, n_adv - n_none) + ["no-answer"] * n_none
        lines.append(f"- {n_adv} adversarial: {', '.join(kinds)}. Swap a type other than no-answer for another if this repository cannot support it.")
    lines.append(f"- {q['ordinary_and_adversarial'] - n_adv} ordinary queries, mixing the other categories.")
    about = run.repos()[repo].get("about") or ABOUT.get(repo, "")
    return PROMPT.format(repo=repo, about=about, n=n, quota="\n".join(lines))


ABOUT: dict[str, str] = {}


def load_about() -> None:
    """The comment above each repository in corpus.toml, as its description."""
    comment = None
    for line in (run.HERE / "corpus.toml").read_text().splitlines():
        if line.startswith("# ") and not line.startswith("# `"):
            comment = line[2:]
        elif line.startswith("[repos."):
            ABOUT[line[len("[repos."):-1].strip('"')] = comment or ""
            comment = None


def parse(text: str) -> list[dict]:
    text = text.strip()
    if text.startswith("```"):
        text = text.split("\n", 1)[1].rsplit("```", 1)[0]
    return tomllib.loads(text).get("query", [])


def check(repo: str, text: str) -> tuple[list[dict], list[str]]:
    """The draft's queries and what is wrong with them."""
    try:
        qs = parse(text)
    except tomllib.TOMLDecodeError as e:
        return [], [f"the reply is not valid TOML: {e}"]
    bad = []
    seen = set()
    for q in qs:
        for p in run.problems(q):
            bad.append(f"{q.get('id')}: {p}")
        for a in q.get("answers", []):
            if a.get("repo") != repo:
                bad.append(f"{q.get('id')}: answers are in {repo} only, not {a.get('repo')}")
        if q.get("id") in seen:
            bad.append(f"{q.get('id')}: the id is used twice")
        seen.add(q.get("id"))
    return qs, bad


FIELDS = ("id", "text", "category", "adversarial", "lang", "by", "note", "review")
ANSWER_FIELDS = ("repo", "path", "line", "kind", "anchor")


def toml_string(v) -> str:
    # JSON's string escapes are valid in a TOML basic string.
    return json.dumps(v, ensure_ascii=False) if isinstance(v, str) else str(v)


def toml_query(q: dict) -> str:
    """One query as a [[query]] table, fields in a fixed order."""
    lines = ["[[query]]"]
    lines += [f"{k} = {toml_string(q[k])}" for k in FIELDS if q.get(k) is not None]
    if not q["answers"]:
        lines.append("answers = []")
    else:
        lines.append("answers = [")
        for a in q["answers"]:
            lines.append("  { " + ", ".join(f"{k} = {toml_string(a[k])}" for k in ANSWER_FIELDS if k in a) + " },")
        lines.append("]")
    return "\n".join(lines) + "\n"


def clean_env() -> dict:
    # A session started from inside Claude Code must not inherit its effort,
    # messaging socket or session (agent.py, `claude`).
    return {k: v for k, v in os.environ.items() if not k.startswith(("CLAUDE", "ANTHROPIC"))}


def claude(repo: str, text: str, session: str | None, log: Path) -> tuple[str, str]:
    argv = ["claude", "-p", "--model", CLAUDE_MODEL, "--effort", CLAUDE_EFFORT, "--setting-sources", "",
            "--disable-slash-commands", "--tools", "Read,Grep,Glob", "--allowedTools", "Read,Grep,Glob",
            "--max-turns", "200", "--output-format", "stream-json", "--verbose"]
    if session:
        argv += ["--resume", session]
    p = subprocess.run([*argv, text], cwd=run.PROJECTS / repo, env=clean_env(), capture_output=True, text=True,
                       timeout=7200, stdin=subprocess.DEVNULL)
    with log.open("a") as f:
        f.write(p.stdout)
    events = [json.loads(l) for l in p.stdout.splitlines() if l.startswith("{")]
    result = next((e for e in events if e.get("type") == "result"), None)
    if result is None:
        raise RuntimeError(f"claude exited {p.returncode}: {p.stderr.strip()[-500:]}")
    if result.get("is_error"):  # a usage limit (429) arrives as the reply text
        raise RuntimeError(f"claude: {result.get('result')}")
    return result.get("result") or "", result["session_id"]


def codex(repo: str, text: str, session: str | None, log: Path) -> tuple[str, str]:
    out = log.with_suffix(".last.txt")
    flags = ["-m", CODEX_MODEL, "-c", f'model_reasoning_effort="{CODEX_EFFORT}"', "-c", 'web_search="disabled"', "-c", 'sandbox_mode="read-only"',
             "--skip-git-repo-check", "--json", "-o", str(out)]
    for feature in CODEX_OFF:
        flags += ["--disable", feature]
    if session:
        argv = ["codex", "exec", "resume", *flags, session, text]
    else:
        argv = ["codex", "exec", "-C", str(run.PROJECTS / repo), *flags, text]
    p = subprocess.run(argv, cwd=run.PROJECTS / repo, capture_output=True, text=True, timeout=7200, stdin=subprocess.DEVNULL)
    with log.open("a") as f:
        f.write(p.stdout)
    events = [json.loads(l) for l in p.stdout.splitlines() if l.startswith("{")]
    started = next((e for e in events if e.get("type") == "thread.started"), None)
    if p.returncode or not out.exists():
        raise RuntimeError(f"codex exited {p.returncode}: {p.stderr.strip()[-500:]}")
    return out.read_text(), session or (started or {}).get("thread_id")


def write_one(writer: str, repo: str) -> str:
    target = DRAFTS / writer / f"{repo}.toml"
    if target.exists():
        return f"{writer} {repo}: done before"
    target.parent.mkdir(parents=True, exist_ok=True)
    LOGS.mkdir(parents=True, exist_ok=True)
    log = LOGS / f"{writer}-{repo}.jsonl"
    call = claude if writer == "claude" else codex
    try:
        reply, session = call(repo, prompt(repo, writer), None, log)
        qs, bad = check(repo, reply)
        for _ in range(FIX_ROUNDS):
            if not bad:
                break
            fix = ("These problems were found in your queries:\n- " + "\n- ".join(bad) +
                   "\n\nFix them (re-read the files for exact anchors and line numbers) and reply with the complete corrected TOML for all queries, nothing else.")
            reply, session = call(repo, fix, session, log)
            qs, bad = check(repo, reply)
    except Exception as e:  # noqa: BLE001, one repository's failure does not stop the others
        return f"{writer} {repo}: FAILED {e}"
    model = f"{CLAUDE_MODEL}, effort {CLAUDE_EFFORT}" if writer == "claude" else f"{CODEX_MODEL}, effort {CODEX_EFFORT}"
    header = f"# Draft queries for {repo} by {writer} ({model}), written {run.datetime.now():%Y-%m-%d}. Not reviewed.\n\n"
    if bad:
        target.with_suffix(".failed.toml").write_text(header + "".join(f"# PROBLEM {b}\n" for b in bad) + "\n" + reply.strip() + "\n")
        return f"{writer} {repo}: {len(qs)} queries, {len(bad)} problems left, kept as .failed.toml"
    target.write_text(header + reply.strip() + "\n")
    return f"{writer} {repo}: {len(qs)} queries"


def review_one(reviewer: str, repo: str) -> str:
    author = "codex" if reviewer == "claude" else "claude"
    draft = DRAFTS / author / f"{repo}.toml"
    target = DRAFTS / author / f"{repo}.reviewed.toml"
    if target.exists():
        return f"{reviewer} reviewing {author}/{repo}: done before"
    if not draft.exists():
        return f"{reviewer} reviewing {author}/{repo}: no draft"
    rules = PROMPT.split("## What a query is", 1)[1].split("## Output", 1)[0]
    text = REVIEW.format(repo=repo, about=ABOUT.get(repo, ""), rules="## What a query is" + rules,
                         draft=draft.read_text().split("\n", 2)[2])
    log = LOGS / f"review-{reviewer}-{repo}.jsonl"
    call = claude if reviewer == "claude" else codex
    before = {q["id"] for q in parse(draft.read_text().split("\n", 2)[2])}
    try:
        reply, session = call(repo, text, None, log)
        bad = verdict_problems(repo, reply, before)
        for _ in range(FIX_ROUNDS):
            if not bad:
                break
            fix = ("These problems were found in your reviewed queries:\n- " + "\n- ".join(bad) +
                   "\n\nFix them and reply with the complete TOML for all queries, nothing else.")
            reply, session = call(repo, fix, session, log)
            bad = verdict_problems(repo, reply, before)
    except Exception as e:  # noqa: BLE001
        return f"{reviewer} reviewing {author}/{repo}: FAILED {e}"
    model = f"{CLAUDE_MODEL}, effort {CLAUDE_EFFORT}" if reviewer == "claude" else f"{CODEX_MODEL}, effort {CODEX_EFFORT}"
    header = f"# {author}'s queries for {repo}, reviewed by {reviewer} ({model}) on {run.datetime.now():%Y-%m-%d}.\n\n"
    if bad:
        target.with_suffix(".failed.toml").write_text(header + "".join(f"# PROBLEM {b}\n" for b in bad) + "\n" + reply.strip() + "\n")
        return f"{reviewer} reviewing {author}/{repo}: {len(bad)} problems left, kept as .failed.toml"
    target.write_text(header + reply.strip() + "\n")
    qs = parse(reply)
    tally = {v: sum(1 for q in qs if q["review"].split(":")[0] == v) for v in ("keep", "fixed", "drop")}
    return f"{reviewer} reviewing {author}/{repo}: {tally}"


def verdict_problems(repo: str, reply: str, before: set[str]) -> list[str]:
    """`check`'s problems for the queries not dropped, and a verdict on each."""
    try:
        qs = parse(reply)
    except tomllib.TOMLDecodeError as e:
        return [f"the reply is not valid TOML: {e}"]
    bad = []
    if {q.get("id") for q in qs} != before:
        bad.append(f"the ids changed: expected {sorted(before)}")
    for q in qs:
        v = str(q.get("review", ""))
        if not (v == "keep" or v.startswith("fixed:") or v.startswith("drop:")):
            bad.append(f"{q.get('id')}: review must be keep, fixed: ... or drop: ...")
    kept = "\n".join(toml_query(q) for q in qs if not str(q.get("review", "")).startswith("drop"))
    return bad + check(repo, kept)[1]


def review(args) -> None:
    run.verify_checkout()
    load_about()
    repos = args.repo or sorted(run.repos(), key=lambda r: -json.loads((run.HERE / "corpus-sizes.json").read_text())[r])
    reviewers = [args.reviewer] if args.reviewer else ["claude", "codex"]
    jobs = [(w, r) for r in repos for w in reviewers]
    with ThreadPoolExecutor(args.parallel * len(reviewers)) as pool:
        for line in pool.map(lambda j: review_one(*j), jobs):
            print(line, flush=True)


DUPLICATE_COSINE = 0.85
NEAR_LINES = 3


def embed(texts: list[str]) -> list[list[float]]:
    """Octen vectors, as hister embeds a query: the plain text."""
    out = []
    for i in range(0, len(texts), 16):
        data, _ = run.request(f"{run.INFERMUX}/v1/embeddings", {"model": run.EMBEDDER, "input": texts[i : i + 16]})
        out += [d["embedding"] for d in sorted(data["data"], key=lambda d: d["index"])]
    return out


def cosine(a: list[float], b: list[float]) -> float:
    dot = sum(x * y for x, y in zip(a, b))
    return dot / (math.sqrt(sum(x * x for x in a)) * math.sqrt(sum(y * y for y in b)))


def same_place(a: dict, b: dict) -> bool:
    """Two queries share an answer: the same file, and for text answers lines
    within NEAR_LINES of each other."""
    for x in a["answers"]:
        for y in b["answers"]:
            if x["repo"] == y["repo"] and x["path"] == y["path"]:
                if "line" not in x or "line" not in y or abs(x["line"] - y["line"]) <= NEAR_LINES:
                    return True
    return False


def merge(args) -> None:
    """Per repository: the reviewed queries of both writers minus the dropped,
    duplicates merged (a shared answer place and Octen cosine >= 0.85; the
    one kept is the one its reviewer kept unchanged, else the writer with
    fewer queries kept so far), into bench/queries/<repo>.toml."""
    run.verify_checkout()
    report = ["# Queries merged as duplicates", "", f"Rule: a shared answer place and Octen cosine >= {DUPLICATE_COSINE}.", ""]
    dropped = ["", "# Queries reviewers dropped", ""]
    totals = {"kept": 0, "duplicates": 0, "dropped": 0}
    for repo in sorted(run.repos()):
        qs = []
        for author in ("claude", "codex"):
            f = DRAFTS / author / f"{repo}.reviewed.toml"
            if not f.exists():
                continue
            for q in parse(f.read_text().split("\n", 2)[2]):
                if q["review"].startswith("drop"):
                    dropped.append(f"- {repo}, {author}, {q['id']}: {q['text']} ({q['review']})")
                    totals["dropped"] += 1
                    continue
                qs.append(q | {"by": author})
        if not qs:
            continue
        vectors = embed([q["text"] for q in qs])
        kept: list[int] = []
        for i, q in enumerate(qs):
            twin = next((j for j in kept if same_place(q, qs[j]) and cosine(vectors[i], vectors[j]) >= DUPLICATE_COSINE), None)
            if twin is None:
                kept.append(i)
                continue
            other = qs[twin]
            clean = lambda x: x["review"] == "keep"  # noqa: E731
            counts = {w: sum(1 for j in kept if qs[j]["by"] == w) for w in ("claude", "codex")}
            prefer_new = (clean(q) and not clean(other)) or (clean(q) == clean(other) and counts[q["by"]] < counts[other["by"]])
            if prefer_new:
                kept[kept.index(twin)] = i
                q, other = other, q
            report.append(f"- {repo}: kept {other['by']} \"{other['text']}\"; merged {q['by']} \"{q['text']}\" "
                          f"(cosine {cosine(vectors[qs.index(q)], vectors[qs.index(other)]):.2f})")
            totals["duplicates"] += 1
        out = run.QUERIES / f"{repo}.toml"
        existing = tomllib.loads(out.read_text()).get("query", []) if out.exists() else []
        ids = {q["id"] for q in existing}
        new = []
        for j in kept:
            q = {k: v for k, v in qs[j].items() if k != "review"}
            if q["id"] in ids:
                q["id"] = f"{q['id']}-{q['by']}"
            ids.add(q["id"])
            new.append(q)
        head = out.read_text() if out.exists() else f"# Queries about {repo}, for bench/run.py. Format and relevance rule: bench/README.md, \"Quality\".\n"
        out.write_text(head.rstrip("\n") + "\n\n# Written by Claude and Codex and cross-reviewed, 2026-10-07 (bench/write_queries.py).\n\n"
                       + "\n".join(toml_query(q) for q in new))
        totals["kept"] += len(new)
    (DRAFTS / "merged.md").write_text("\n".join(report + dropped) + "\n")
    print(totals)


def write(args) -> None:
    run.verify_checkout()
    load_about()
    repos = args.repo or sorted(run.repos(), key=lambda r: -json.loads((run.HERE / "corpus-sizes.json").read_text())[r])
    writers = [args.writer] if args.writer else ["claude", "codex"]
    jobs = [(w, r) for r in repos for w in writers]
    with ThreadPoolExecutor(args.parallel * len(writers)) as pool:
        for line in pool.map(lambda j: write_one(*j), jobs):
            print(line, flush=True)


def status(args) -> None:
    load_about()
    q = quotas()
    for writer in ("claude", "codex"):
        done = sorted(p.stem for p in (DRAFTS / writer).glob("*.toml") if not p.stem.endswith(".failed"))
        failed = sorted(p.stem.removesuffix(".failed") for p in (DRAFTS / writer).glob("*.failed.toml"))
        n = sum(len(parse((DRAFTS / writer / f"{r}.toml").read_text().split("\n", 2)[2])) for r in done)
        print(f"{writer}: {len(done)}/{len(q)} repositories, {n} queries; failed: {', '.join(failed) or 'none'}")


def main() -> None:
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("write")
    s.set_defaults(fn=write)
    s.add_argument("--writer", choices=("claude", "codex"))
    s.add_argument("--repo", action="append")
    s.add_argument("--parallel", type=int, default=4, help="sessions per writer at a time")
    s = sub.add_parser("review")
    s.set_defaults(fn=review)
    s.add_argument("--reviewer", choices=("claude", "codex"))
    s.add_argument("--repo", action="append")
    s.add_argument("--parallel", type=int, default=4, help="sessions per reviewer at a time")
    s = sub.add_parser("merge")
    s.set_defaults(fn=merge)
    s = sub.add_parser("status")
    s.set_defaults(fn=status)
    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
