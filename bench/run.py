# /// script
# requires-python = ">=3.12"
# ///
"""Benchmarks for code search, against a seeded hister instance of its own.
See bench/README.md.

    uv run bench/run.py checkout     clone the pinned corpus
    uv run bench/run.py seed         clone the pinned corpus and index it
    uv run bench/run.py check        every answer's line and the pieces that hold it
    uv run bench/run.py quality      ranks of the known answers in queries.toml
    uv run bench/run.py latency      per-stage time of each query, from InferMux's records
    uv run bench/run.py throughput --target zbox|local

Each run writes bench/results/<time>-<kind>.json and prints a summary.
"""

import argparse
import contextlib
import hashlib
import json
import os
import random
import shutil
import socket
import sqlite3
import statistics
import subprocess
import sys
import time
import tomllib
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
from pathlib import Path

HERE = Path(__file__).parent
REPO = HERE.parent
SOURCES = Path.home() / "Projects"
# Everything the seeded instance owns. Nothing here touches the real one.
SHARE = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share")) / "semsearch"
# SEMSEARCH_BENCH moves it, so a second embedder gets an instance of its own.
# bench-v2 holds the corpus of 2026-10-07; `bench` the six repositories before it.
BENCH = Path(os.environ.get("SEMSEARCH_BENCH", SHARE / "bench-v2"))
PROJECTS = BENCH / "corpus"  # the pinned checkouts, one per repository
XDG = BENCH / "xdg"  # semsearch's state file goes under this XDG_DATA_HOME
DATA = XDG / "semsearch"
HISTER_DATA = BENCH / "hister"
PORT = 4440
HISTER = f"http://127.0.0.1:{PORT}"
INFERMUX = "http://127.0.0.1:5001"
# Seeding embeds on reaperboi's card, past the failover (the user, 2026-10-06).
# Queries go the normal way, to the zbox. hister keeps the seeding endpoint's
# fingerprint and warns at each start that the query endpoint differs.
SEED_ENDPOINT = f"{INFERMUX}/upstream/Octen-Embedding-4B.Q8_0/v1/embeddings"
CLIENT = "semsearch"  # hister's key, as InferMux records it
EMBEDDER = "Octen-Embedding-4B.Q8_0"
# hister's settings in config/hister.yml, so the throughput run sends what it does.
EMBED_BATCH = 8
EMBED_CONCURRENCY = 2
CUTOFFS = (1, 3, 10)


def secrets() -> dict[str, str]:
    """hister's InferMux keys, as the real instance gets them (nixcfg)."""
    out = {}
    for line in Path("/run/secrets/rendered/semsearch.env").read_text().splitlines():
        name, _, value = line.partition("=")
        out[name] = value
    return out


def key() -> str:
    return os.environ.get("HISTER__SEMANTIC_SEARCH__API_KEY") or secrets()["HISTER__SEMANTIC_SEARCH__API_KEY"]


# The seeded instance


def repos() -> dict[str, dict]:
    return tomllib.loads((HERE / "corpus.toml").read_text())["repos"]


def pins() -> dict[str, str]:
    return {name: r["pin"] for name, r in repos().items()}


def git(*args: str) -> str:
    return subprocess.run(["git", *args], check=True, capture_output=True, text=True).stdout.strip()


def sparse(r: dict) -> list[str]:
    """The repository's sparse-checkout patterns, or none for all of it.
    `keep` re-includes paths under an excluded directory's children, such as
    the source language among translations."""
    if not r.get("include") and not r.get("exclude"):
        return []
    return list(r.get("include") or ["/*"]) + [f"!{p}" for p in r.get("exclude", [])] + list(r.get("keep", []))


def excluded_bytes(target: Path) -> tuple[int, int]:
    """Files and bytes the sparse checkout leaves out at HEAD, so a wrong
    exclusion shows as a number. Sizes come from the tree, which fetches the
    left-out blobs of a blobless clone once."""
    skipped = [line[2:] for line in git("-C", str(target), "ls-files", "-t").splitlines() if line.startswith("S ")]
    if not skipped:
        return 0, 0
    sizes = {}
    for line in git("-C", str(target), "ls-tree", "-r", "-l", "HEAD").splitlines():
        meta, path = line.split("\t", 1)
        sizes[path] = int(meta.split()[3]) if meta.split()[3] != "-" else 0
    return len(skipped), sum(sizes.get(p, 0) for p in skipped)


def checkout(args=None) -> None:
    """Each repository at its pin: cloned blobless from its `url`, or from
    ~/Projects, and checked out sparse when it has `include` or `exclude`."""
    PROJECTS.mkdir(parents=True, exist_ok=True)
    for repo, r in repos().items():
        target = PROJECTS / repo
        if not target.exists():
            if r.get("url"):
                git("clone", "-q", "--filter=blob:none", "--no-checkout", r["url"], str(target))
            else:
                git("clone", "-q", "--shared", "--no-checkout", str(SOURCES / repo), str(target))
        if subprocess.run(["git", "-C", str(target), "cat-file", "-e", f"{r['pin']}^{{commit}}"], capture_output=True).returncode:
            git("-C", str(target), "fetch", "-q", "origin")
        if patterns := sparse(r):
            git("-C", str(target), "sparse-checkout", "set", "--no-cone", *patterns)
        else:
            git("-C", str(target), "sparse-checkout", "disable")
        git("-C", str(target), "checkout", "-q", "--force", "--detach", r["pin"])
        git("-C", str(target), "clean", "-q", "-fdx")
        if repo == "SemSearch":
            shutil.rmtree(target / "bench", ignore_errors=True)
        files, size = excluded_bytes(target)
        print(f"{repo} at {r['pin'][:10]}" + (f", {files} files and {size / 1e6:.2f} MB excluded" if files else ""), file=sys.stderr)


def verify_checkout() -> None:
    for repo, commit in pins().items():
        head = git("-C", str(PROJECTS / repo), "rev-parse", "HEAD") if (PROJECTS / repo).exists() else None
        if head != commit:
            sys.exit(f"{repo} is at {head}, its pin is {commit}: run `bench/run.py seed`")


def config() -> Path:
    """config/hister.yml, with this instance's data directory and port."""
    lines = []
    for line in (REPO / "config/hister.yml").read_text().splitlines():
        if line.startswith("  directory:"):
            line = f"  directory: {HISTER_DATA}"
        elif line.startswith("  address:"):
            line = f"  address: 127.0.0.1:{PORT}"
        lines.append(line)
    out = BENCH / "hister.yml"
    out.write_text("\n".join(lines) + "\n")
    return out


@contextlib.contextmanager
def instance(seeding: bool = False):
    """The seeded hister, running for the duration, stopped by its PID."""
    with socket.socket() as s:
        if s.connect_ex(("127.0.0.1", PORT)) == 0:
            sys.exit(f"something already listens on :{PORT}")
    env = os.environ | secrets()
    # An endpoint set in the environment wins, so another embedder can seed
    # its own instance (SEMSEARCH_BENCH apart) and answer its queries too.
    if seeding:
        env.setdefault("HISTER__SEMANTIC_SEARCH__EMBEDDING_ENDPOINT", SEED_ENDPOINT)
    HISTER_DATA.mkdir(parents=True, exist_ok=True)
    log = open(BENCH / "hister.log", "a")
    proc = subprocess.Popen([shutil.which("hister"), "--config", str(config()), "listen"], env=env, stdout=log, stderr=log)
    try:
        for _ in range(300):
            if proc.poll() is not None:
                sys.exit(f"hister exited with {proc.returncode}, see {BENCH / 'hister.log'}")
            with socket.socket() as s:
                if s.connect_ex(("127.0.0.1", PORT)) == 0:
                    break
            time.sleep(0.2)
        else:
            sys.exit("hister did not start listening in 60 s")
        yield proc
    finally:
        proc.terminate()
        try:
            proc.wait(30)
        except subprocess.TimeoutExpired:
            proc.kill()
        log.close()


def seed(args) -> None:
    checkout()
    with instance(seeding=True):
        env = os.environ | {"SEMSEARCH_URL": HISTER, "XDG_DATA_HOME": str(XDG)}
        subprocess.run(["semsearch", "index-code", str(PROJECTS)], env=env, check=True)
        start = time.time()
        while (n := backlog()) > 0:
            print(f"{time.time() - start:6.0f} s: {n} documents left to embed", file=sys.stderr)
            time.sleep(30)
    print(f"seeded in {time.time() - start:.0f} s after indexing", file=sys.stderr)


def request(url: str, body: dict | None = None, timeout: float = 600) -> tuple[dict, float]:
    headers = {"Authorization": f"Bearer {key()}"}
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    req = urllib.request.Request(url, data, headers)
    start = time.perf_counter()
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        out = json.load(resp)
    return out, time.perf_counter() - start


def search(text: str) -> tuple[dict, float, datetime]:
    q = json.dumps({"text": f"type:code {text}", "semantic_enabled": True})
    req = urllib.request.Request(f"{HISTER}/search?" + urllib.parse.urlencode({"query": q}), headers={"Origin": "hister://"})
    started = datetime.now(timezone.utc)
    start = time.perf_counter()
    with urllib.request.urlopen(req, timeout=120) as resp:
        data = json.load(resp)
    return data, time.perf_counter() - start, started


def backlog(data: Path = HISTER_DATA) -> int:
    """Documents waiting for vectors in an instance."""
    db = sqlite3.connect(f"file:{data / 'db.sqlite3'}?mode=ro", uri=True)
    return db.execute("select count(*) from embedding_jobs").fetchone()[0]


def real_backlog() -> int:
    """The real instance's, which shares the embedder with the queries."""
    return backlog(SHARE / "hister")


def setup() -> dict:
    """What a result depends on besides the queries, to compare runs by."""
    def digest(p: Path) -> str:
        return hashlib.sha256(p.read_bytes()).hexdigest()[:16]

    return {"pins": pins(), "hister": os.path.realpath(shutil.which("hister")),
            "semsearch": os.path.realpath(shutil.which("semsearch")),
            "hister_config_sha256": digest(BENCH / "hister.yml"), "queries_sha256": queries_digest(),
            # hister settings overridden from the environment, keys left out.
            "hister_env": {k: v for k, v in sorted(os.environ.items()) if k.startswith("HISTER__") and "KEY" not in k},
            "real_backlog": real_backlog()}


def save(kind: str, result: dict) -> Path:
    out = HERE / "results" / f"{datetime.now():%Y-%m-%d-%H%M%S}-{kind}.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps(result, indent=1, ensure_ascii=False) + "\n")
    return out


def summarize(xs: list[float]) -> dict | None:
    """Median and the 90th percentile (statistics.quantiles, exclusive method)."""
    if not xs:
        return None
    out = {"n": len(xs), "median": round(statistics.median(xs), 3), "max": round(max(xs), 3)}
    if len(xs) >= 2:
        out["p90"] = round(statistics.quantiles(xs, n=10)[-1], 3)
    return out


QUERIES = HERE / "queries"  # one file per repository, the queries written for it


def queries(split: str | None = None) -> list[dict]:
    """Every query, with `repo` (the file it is in) and that repository's
    `split`; only one split's when `split` is given."""
    corpus = repos()
    out = []
    for f in sorted(QUERIES.glob("*.toml")):
        for q in tomllib.loads(f.read_text()).get("query", []):
            out.append(q | {"repo": f.stem, "split": corpus[f.stem]["split"]})
    return [q for q in out if split is None or q["split"] == split]


def queries_digest() -> str:
    h = hashlib.sha256()
    for f in sorted(QUERIES.glob("*.toml")):
        h.update(f.name.encode() + b"\0" + f.read_bytes())
    return h.hexdigest()[:16]


# What a query and its answers may say (docs/decisions.md, 2026-10-07).
CATEGORIES = ("behavior", "explanation", "usage", "symptom", "config", "media")
ADVERSARIAL = ("decoy", "near-miss", "negation", "no-answer", "short", "verbose", "typo", "language", "injection")
TEXT_KINDS = ("code", "test", "doc", "config")
MEDIA_KINDS = {"image": {"png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "svg", "tif", "tiff", "avif"},
               "audio": {"wav", "mp3", "ogg", "oga", "flac", "m4a", "opus", "aac", "aiff"},
               "video": {"mp4", "webm", "mov", "mkv", "avi", "m4v", "ogv"}, "pdf": {"pdf"}}


def problems(q: dict) -> list[str]:
    """What is wrong with a query, against the checked-out corpus."""
    out = []
    for field in ("id", "text", "category", "answers"):
        if field not in q:
            out.append(f"missing {field}")
    if q.get("category") not in CATEGORIES:
        out.append(f"category {q.get('category')!r} is not one of {', '.join(CATEGORIES)}")
    adv = q.get("adversarial")
    if adv is not None and adv not in ADVERSARIAL:
        out.append(f"adversarial {adv!r} is not one of {', '.join(ADVERSARIAL)}")
    if adv is not None and not q.get("note"):
        out.append("an adversarial query needs a note on its trap")
    answers = q.get("answers", [])
    if not answers and adv != "no-answer":
        out.append("no answers, and not marked adversarial = \"no-answer\"")
    if answers and adv == "no-answer":
        out.append("marked no-answer but has answers")
    if q.get("category") == "media" and answers and not any(a.get("kind") in MEDIA_KINDS for a in answers):
        out.append("a media query needs a media answer")
    for a in answers:
        where = f"{a.get('repo')}/{a.get('path')}"
        file = PROJECTS / str(a.get("repo")) / str(a.get("path"))
        if not file.is_file():
            out.append(f"{where}: no such file in the checkout")
            continue
        kind = a.get("kind")
        if kind in MEDIA_KINDS:
            if file.suffix.lower().lstrip(".") not in MEDIA_KINDS[kind]:
                out.append(f"{where}: kind {kind} does not fit the file type")
            if "line" in a or "anchor" in a:
                out.append(f"{where}: a media answer is the whole file, without line or anchor")
        elif kind in TEXT_KINDS:
            if not isinstance(a.get("line"), int) or not isinstance(a.get("anchor"), str) or not a["anchor"].strip():
                out.append(f"{where}: needs an integer line and a non-empty anchor")
            elif locate(file, a) is None:
                out.append(f"{where}: no line reads {a['anchor']!r} (expected near line {a['line']})")
        else:
            out.append(f"{where}: kind {kind!r} is not one of {', '.join(TEXT_KINDS + tuple(MEDIA_KINDS))}")
    return out


# Answers and pieces


class Index:
    """semsearch's pieces, from its state file, and which of them have vectors."""

    def __init__(self) -> None:
        if not (DATA / "code-state.json").exists():
            self.files, self.embedded = {}, set()
            return
        state = json.loads((DATA / "code-state.json").read_text())
        # Per file, its pieces in file order as (line, url).
        self.files: dict[str, list[tuple[int, str]]] = {
            path: [(int(u.rsplit(":", 2)[1]), u) for u in f["urls"]] for path, f in state["files"].items()
        }
        vectors = sqlite3.connect(f"file:{HISTER_DATA / 'vectors.sqlite3'}?mode=ro", uri=True)
        self.embedded = {r[0] for r in vectors.execute("select distinct doc_id from chunk_meta where doc_id like 'vscode://%'")}

    def holding(self, path: str, line: int) -> set[str]:
        """The URLs of the pieces whose span holds `line`. A piece runs from its
        first line to the next piece's first line, which may be the same."""
        pieces = self.files.get(path, [])
        out = set()
        for i, (start, url) in enumerate(pieces):
            end = pieces[i + 1][0] if i + 1 < len(pieces) else float("inf")
            if start <= line <= end:
                out.add(url)
        return out


def locate(file: Path, answer: dict) -> int | None:
    """The line of `file` holding the answer's anchor text, the nearest to the
    answer's line when several do, or None when none does."""
    lines = file.read_text(errors="replace").splitlines()
    same = [i + 1 for i, l in enumerate(lines) if l.strip() == answer["anchor"].strip()]
    return min(same, key=lambda n: abs(n - answer["line"])) if same else None


def checkouts(repo: str) -> list[str]:
    return [repo] + sorted(p.name for p in PROJECTS.glob(f"{repo}-*") if (p / ".git").exists())


def acceptable(answer: dict, index: Index) -> dict[str, str]:
    """Every piece that counts as this answer, URL -> checkout. In every
    checkout the answer is where its anchor text is (`locate`), so it follows
    the code as lines move."""
    out = {}
    if answer["kind"] in MEDIA_KINDS:
        return out  # nothing embeds media yet (docs/decisions.md, 2026-10-07)
    for checkout in checkouts(answer["repo"]):
        file = PROJECTS / checkout / answer["path"]
        if not file.exists() or (line := locate(file, answer)) is None:
            continue
        for url in index.holding(str(file), line):
            out[url] = checkout
    return out


def check(args) -> None:
    """Every query's problems (`problems`), and for each text answer the
    pieces that hold it once the instance is seeded."""
    verify_checkout()
    index = Index()
    bad = 0
    ids: dict[str, str] = {}
    for q in queries():
        print(f"{q['repo']}/{q['id']}: {q['text']}")
        if q["id"] in ids:
            print(f"  ERROR the id is used in {ids[q['id']]} too")
            bad += 1
        ids[q["id"]] = q["repo"]
        for p in problems(q):
            print(f"  ERROR {p}")
            bad += 1
        for a in q["answers"]:
            if a["kind"] in MEDIA_KINDS:
                print(f"  {a['kind']:6} {a['repo']}/{a['path']}  (not indexed: nothing embeds media)")
                continue
            file = PROJECTS / a["repo"] / a["path"]
            line = locate(file, a) if file.exists() else None
            if line is None:
                continue
            moved = f" (moved from {a['line']})" if line != a["line"] else ""
            print(f"  {a['kind']:6} {a['repo']}/{a['path']}:{line}{moved}  {a['anchor'][:90]}")
            if index.files:
                pieces = acceptable(a, index)
                embedded = sum(u in index.embedded for u in pieces)
                print(f"         {len(pieces)} pieces in {len(set(pieces.values()))} checkouts, {embedded} embedded")
                if not pieces:
                    bad += 1
    print(f"{len(ids)} queries, {bad} problems")
    sys.exit(1 if bad else 0)


# Quality


def orders(resp: dict) -> dict[str, list[str]]:
    """Each order the response gives, as URLs, best first. `final` is
    semsearch's (src/hister.rs, search), including its fallbacks."""
    keyword = [d["url"] for d in resp.get("documents") or []]
    semantic = sorted(resp.get("semantic_hits") or [], key=lambda h: -h["similarity"])
    sim = [(h.get("document") or {}).get("url") or h["doc_id"] for h in semantic]
    reranked = [r["url"] for r in resp.get("reranked") or []]
    if reranked:
        rerank_ids = {r["doc_id"] for r in resp["reranked"]}
        final = reranked + [u for u, h in zip(sim, semantic) if h["doc_id"] not in rerank_ids]
    elif resp.get("semantic_error"):
        final = keyword
    else:
        final = sim
    return {"keyword": keyword, "similarity": sim, "reranked": reranked, "final": final}


def quality(args) -> None:
    verify_checkout()
    with instance():
        run_quality(args)


# The groups every quality summary is broken down by.
GROUPS = {
    "split": lambda q: q["split"],
    "category": lambda q: q["category"],
    "adversarial": lambda q: q.get("adversarial") or "none",
    "writer": lambda q: q.get("by", "agent 2026-10-06"),
    "lang": lambda q: q.get("lang", "en"),
}


def separation(none_scores: list[float], hit_scores: list[float]) -> float | None:
    """How often a no-answer query's best rerank score is below the rerank
    score of an answerable query's first correct hit, over all such pairs."""
    if not none_scores or not hit_scores:
        return None
    below = sum((n < h) + 0.5 * (n == h) for n in none_scores for h in hit_scores)
    return round(below / (len(none_scores) * len(hit_scores)), 3)


def run_quality(args) -> None:
    index = Index()
    rows = []
    for q in queries(None if args.split == "all" else args.split):
        resp, took, _ = search(q["text"])
        answers = [(a, acceptable(a, index)) for a in q["answers"]]
        ranked = orders(resp)
        scores = {r["url"]: r.get("rerank_score") for r in resp.get("reranked") or []}

        def first(urls: list[str], kinds: set[str]) -> int | None:
            good = {u for a, pieces in answers if a["kind"] in kinds for u in pieces}
            return next((i + 1 for i, u in enumerate(urls) if u in good), None)

        answerable = bool(q["answers"])
        ranks = {name: first(urls, set(TEXT_KINDS) | set(MEDIA_KINDS)) for name, urls in ranked.items()} if answerable else None
        code_ranks = {name: first(urls, {"code"}) for name, urls in ranked.items()} if answerable else None
        hit = ranks and ranks["reranked"]
        pieces = [set(p) for _, p in answers]
        rows.append({
            "id": q["id"], "repo": q["repo"], "text": q["text"], "answers": q["answers"],
            **{g: f(q) for g, f in GROUPS.items()},
            "ranks": ranks, "code_ranks": code_ranks,
            "media": any(a["kind"] in MEDIA_KINDS for a in q["answers"]),
            "top_rerank_score": max(scores.values(), default=None),
            "hit_rerank_score": scores.get(ranked["reranked"][hit - 1]) if hit else None,
            "any_answer_embedded": any(u in index.embedded for p in pieces for u in p),
            "every_answer_embedded": all(any(u in index.embedded for u in p) for p in pieces),
            "semantic_error": resp.get("semantic_error"), "rerank_error": resp.get("rerank_error"),
            "seconds": round(took, 3), "orders": ranked,
        })
        shown = " ".join(f"{k[:4]}={v or '-':>3}" for k, v in ranks.items()) if ranks else f"no answer, top score {rows[-1]['top_rerank_score']}"
        print(f"{q['repo'] + '/' + q['id']:48} {shown}", file=sys.stderr)

    answerable = [r for r in rows if r["ranks"] is not None]
    unanswerable = [r for r in rows if r["ranks"] is None]

    def metrics(rs: list[dict], field: str) -> dict:
        out = {}
        for name in ("keyword", "similarity", "reranked", "final"):
            xs = [r[field][name] for r in rs]
            m = {"n": len(xs)} | {f"hits@{k}": sum(1 for x in xs if x and x <= k) for k in CUTOFFS}
            m |= {f"rate@{k}": round(m[f"hits@{k}"] / len(xs), 3) if xs else None for k in CUTOFFS}
            m["mrr"] = round(sum(1 / x for x in xs if x) / len(xs), 3) if xs else None
            out[name] = m
        return out

    groups = {g: {v: metrics([r for r in answerable if r[g] == v], "ranks")["final"]
                  for v in sorted({r[g] for r in answerable})} for g in GROUPS}
    media = [r for r in answerable if r["media"]]
    none = {"n": len(unanswerable),
            "separation": separation([r["top_rerank_score"] for r in unanswerable if r["top_rerank_score"] is not None],
                                     [r["hit_rerank_score"] for r in answerable if r["hit_rerank_score"] is not None]),
            "median_top_score": summarize([r["top_rerank_score"] for r in unanswerable if r["top_rerank_score"] is not None])}
    lengths = {name: max(len(r["orders"][name]) for r in rows) for name in rows[0]["orders"]}
    result = {"kind": "quality", "when": datetime.now().isoformat(timespec="seconds"), "note": args.note, "split": args.split,
              "setup": setup(), "backlog": backlog(), "queries": len(rows), "list_lengths": lengths,
              "summary": metrics(answerable, "ranks"), "summary_code_only": metrics(answerable, "code_ranks"),
              "groups": groups, "media": {"n": len(media), "answers_not_indexed": len(media)}, "no_answer": none, "rows": rows}
    print(f"\n{len(rows)} queries ({args.split}): {len(answerable)} with answers, {len(unanswerable)} without. "
          f"Backlog {result['backlog']}. MRR over the returned lists, lengths {lengths}.")
    for title, sm in (("any correct piece", result["summary"]), ("implementation only", result["summary_code_only"])):
        print(f"\n{title}\n| order | " + " | ".join(f"hit@{k}" for k in CUTOFFS) + " | MRR |\n|---|" + "---|" * (len(CUTOFFS) + 1))
        for name, m in sm.items():
            print(f"| {name} | " + " | ".join(f"{m[f'hits@{k}']}/{m['n']}" for k in CUTOFFS) + f" | {m['mrr']} |")
    print("\nfinal order by group\n| group | value | n | hit@1 | hit@10 | MRR |\n|---|---|---|---|---|---|")
    for g, vs in groups.items():
        for v, m in vs.items():
            print(f"| {g} | {v} | {m['n']} | {m['rate@1']} | {m['rate@10']} | {m['mrr']} |")
    if media:
        print(f"\nmedia: {len(media)} queries, all counted as misses: nothing embeds images, sound, video or PDFs, so no answer of theirs is indexed")
    print(f"no answer: {none['n']} queries, separation {none['separation']} (share of pairs where a no-answer query's best rerank score "
          f"is below an answerable query's correct hit), median best score {none['median_top_score']}")
    for flag, label in (("any_answer_embedded", "no answer embedded"), ("every_answer_embedded", "some answer not embedded")):
        missing = [r["id"] for r in answerable if not r["media"] and not r[flag]]
        if missing:
            print(f"{label}: {', '.join(missing)}")
    errors = [r["id"] for r in rows if r["semantic_error"] or r["rerank_error"]]
    if errors:
        print(f"degraded searches: {', '.join(errors)}")
    print(save("quality", result))


# Query latency


def calls(since: datetime, until: datetime) -> list[dict]:
    """hister's requests to InferMux that started inside the window."""
    data, _ = request(f"{INFERMUX}/warden/requests?hosts=all")
    out = []
    for host in data["hosts"]:
        for r in host.get("requests") or []:
            t = datetime.fromisoformat(r["time"])
            if r["client"] == CLIENT and since <= t <= until:
                out.append(r | {"host": host.get("host")})
    return out


def loaded() -> list[str]:
    try:
        data, _ = request(f"{INFERMUX}/running")
        return sorted(m.get("model", "?") for m in data.get("running", []))
    except Exception as e:  # noqa: BLE001, the run goes on without it
        return [f"unknown: {e}"]


def latency(args) -> None:
    verify_checkout()
    with instance():
        run_latency(args)


def run_latency(args) -> None:
    qs = queries(None if args.split == "all" else args.split)
    random.Random(args.seed).shuffle(qs)
    before = real_backlog()
    rows = []
    for q in qs:
        models = loaded()
        resp, total, started = search(q["text"])
        ended = datetime.now(timezone.utc)
        time.sleep(0.2)  # the record is written as the reply ends
        mine = calls(started, ended)
        # A query embedding is one short input; the backlog's are batches.
        embeds = [r for r in mine if r["path"] == "/v1/embeddings" and (r.get("prompt_tokens") or 0) < 100]
        reranks = [r for r in mine if r["path"] == "/v1/rerank"]
        embed = embeds[0]["duration_ms"] / 1000 if len(embeds) == 1 else None
        rerank = reranks[0]["duration_ms"] / 1000 if len(reranks) == 1 else None
        rows.append({
            "id": q["id"], "total": round(total, 3), "embed": embed and round(embed, 3), "rerank": rerank and round(rerank, 3),
            "rest": round(total - (embed or 0) - (rerank or 0), 3) if embed is not None and rerank is not None else None,
            "embed_model": embeds[0]["model"] if embeds else None, "loaded_before": models,
            "matched_calls": len(mine), "semantic_error": resp.get("semantic_error"), "rerank_error": resp.get("rerank_error"),
        })
        r = rows[-1]
        print(f"{q['id']:26} total {total:5.2f}  embed {r['embed'] or 0:5.2f}  rerank {r['rerank'] or 0:5.2f}  rest {r['rest'] or 0:5.2f}", file=sys.stderr)

    clean = [r for r in rows if not r["semantic_error"] and not r["rerank_error"]]
    # A query whose reranker had to load first is reported apart.
    cold = [r for r in clean if not any("rerank" in m for m in r["loaded_before"])]
    warm = [r for r in clean if r not in cold]
    summary = {stage: summarize([r[stage] for r in warm if r[stage] is not None]) for stage in ("total", "embed", "rerank", "rest")}
    result = {"kind": "latency", "when": datetime.now().isoformat(timespec="seconds"), "note": args.note, "seed": args.seed,
              "setup": setup(), "backlog": [before, real_backlog()], "degraded": len(rows) - len(clean),
              "cold": cold, "summary_warm": summary, "rows": rows}
    print(f"\nbacklog {before} -> {result['backlog'][1]}; {len(warm)} warm, {len(cold)} with the reranker loading, {result['degraded']} degraded")
    print("| stage | n | median | p90 | max |\n|---|---|---|---|---|")
    for stage, s in summary.items():
        print(f"| {stage} | " + (f"{s['n']} | {s['median']} | {s.get('p90', '-')} | {s['max']}" if s else "0 | - | - | -") + " |")
    for r in cold:
        print(f"cold: {r['id']} total {r['total']} rerank {r['rerank']}")
    print(save("latency", result))


# Embedding endpoint throughput


def sample(n: int, seed: int) -> list[dict]:
    """`n` chunks of code, rebuilt into the text hister sent the embedder
    (vectorstore/embedder.go): the metadata chunk under `document:`, a body
    chunk under its document's title, date and language lines and `content:`.
    The document prefix is empty in config/hister.yml."""
    vectors = sqlite3.connect(f"file:{HISTER_DATA / 'vectors.sqlite3'}?mode=ro", uri=True)
    rows = vectors.execute(
        "select chunk_key, doc_id, chunk_idx, chunk_text from chunk_meta where doc_id like 'vscode://%' order by chunk_key"
    ).fetchall()
    if n <= 0 or n > len(rows):
        sys.exit(f"--chunks must be between 1 and {len(rows)}")
    metadata = {doc: text for _, doc, idx, text in rows if idx == 0}
    out = []
    for chunk_key, doc, idx, text in random.Random(seed).sample(rows, n):
        if idx == 0:
            embedded = "document:\n" + text
        else:
            header = [l for l in metadata.get(doc, "").splitlines() if not l.startswith("url: ")]
            embedded = "\n".join(header) + "\ncontent:\n" + text
        out.append({"key": chunk_key, "metadata": idx == 0, "text": embedded})
    return out


def throughput(args) -> None:
    if args.target == "zbox":
        url, model = f"{INFERMUX}/v1/embeddings", f"zbox/{EMBEDDER}"
    else:
        # /upstream/ goes to this host's llama-swap past the failover.
        url, model = f"{INFERMUX}/upstream/{EMBEDDER}/v1/embeddings", EMBEDDER
    chunks = sample(args.chunks, args.seed)
    _, first = request(url, {"model": model, "input": f"warm up {time.time()}"})
    before = real_backlog()

    def run(part: list[dict]) -> dict:
        texts = [c["text"] for c in part]
        batches = [texts[i : i + EMBED_BATCH] for i in range(0, len(texts), EMBED_BATCH)]
        start = time.perf_counter()
        with ThreadPoolExecutor(EMBED_CONCURRENCY) as pool:
            replies = list(pool.map(lambda b: request(url, {"model": model, "input": b}), batches))
        took = time.perf_counter() - start
        tokens = sum(r["usage"]["prompt_tokens"] for r, _ in replies)
        return {"chunks": len(part), "tokens": tokens, "seconds": round(took, 2), "tokens_per_s": round(tokens / took),
                "chunks_per_s": round(len(part) / took, 2), "batch_seconds": summarize([t for _, t in replies])}

    result = {"kind": "throughput", "when": datetime.now().isoformat(timespec="seconds"), "note": args.note,
              "target": args.target, "seed": args.seed, "batch": EMBED_BATCH, "concurrency": EMBED_CONCURRENCY,
              "first_request_s": round(first, 2),
              # Each chunk is sent once: the embedder's prompt cache would
              # answer a repeat. Metadata chunks first, then body chunks.
              "metadata_chunks": (meta := run([c for c in chunks if c["metadata"]])),
              "body_chunks": (body := run([c for c in chunks if not c["metadata"]])),
              "backlog": [before, real_backlog()],
              "all": {"chunks": meta["chunks"] + body["chunks"], "tokens": (tok := meta["tokens"] + body["tokens"]),
                      "seconds": (sec := round(meta["seconds"] + body["seconds"], 2)), "tokens_per_s": round(tok / sec),
                      "chunks_per_s": round((meta["chunks"] + body["chunks"]) / sec, 2)},
              "sample_sha256": hashlib.sha256("\n".join(c["key"] for c in chunks).encode()).hexdigest()[:16]}
    print(json.dumps({k: result[k] for k in ("target", "first_request_s", "all", "metadata_chunks", "body_chunks", "backlog")}, indent=1))
    print(save(f"throughput-{args.target}", result))


def main() -> None:
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("checkout")
    s.set_defaults(fn=checkout)
    s = sub.add_parser("seed")
    s.set_defaults(fn=seed)
    s = sub.add_parser("check")
    s.set_defaults(fn=check)
    for name, fn in (("quality", quality), ("latency", latency)):
        s = sub.add_parser(name)
        s.set_defaults(fn=fn)
        # Test is for reporting; configurations are tuned on dev (docs/decisions.md, 2026-10-07).
        s.add_argument("--split", choices=("dev", "test", "all"), default="dev")
        s.add_argument("--note", default="", help="conditions worth keeping with the result")
        s.add_argument("--seed", type=int, default=1, help="query order, for latency")
    s = sub.add_parser("throughput")
    s.set_defaults(fn=throughput)
    s.add_argument("--target", choices=("zbox", "local"), required=True)
    s.add_argument("--chunks", type=int, default=96)
    s.add_argument("--seed", type=int, default=1)
    s.add_argument("--note", default="")
    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
