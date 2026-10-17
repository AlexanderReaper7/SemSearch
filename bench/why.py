# /// script
# requires-python = ">=3.12"
# ///
"""Grade the why-lines of saved agent runs with Claude: is each line true of
the code it points at, and does it say why that place answers the query.
Evaluation only: the grades and the prompts quote Claude's output and must
never become training data (docs/deep-search.md, "Training data").

    uv run bench/why.py --corpus ~/.local/share/semsearch/bench/corpus bench/results/2026-10-07-035717-agent.json

Each query is one `claude -p` call holding every hit of every run for it,
with the lines each hit points at. Writes bench/results/<time>-why.json.
"""

import argparse
import json
import statistics
import sys
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime
from pathlib import Path

import agent
import run

CONTEXT = 3  # lines shown around a hit's range
SHOWN = 120  # lines of a hit shown at most
# Directories renamed since a corpus was seeded.
RENAMED = {"Semantic-Search": "SemSearch"}

PROMPT = """You grade the result lines of a code search agent. For a question about a codebase, the agent showed places in the code, each with one line on why it answers the question. Grade each line on two things, from the code shown for it:

- true: "yes" if everything the line says about the code holds for the code shown; "partly" if some of it holds and some does not or cannot be seen there; "no" if it is false of that code.
- why: "yes" if the line says how this code answers the question: it connects what the code does to what was asked. "no" if it only names or restates the code, restates the question, or is too generic to tell.

Grade the line, not whether the place is the best answer. A true line about a place that answers nothing is still true.

Reply with only JSON lines, one per hit, in order: {"hit": N, "true": "yes|partly|no", "why": "yes|no", "note": "a few words"}

Question: {query}

{hits}"""


def lines(corpus: Path, hit: dict) -> str | None:
    parts = hit["path"].split("/", 1)
    file = corpus / RENAMED.get(parts[0], parts[0]) / (parts[1] if len(parts) > 1 else "")
    if not file.is_file():
        return None
    text = file.read_text(errors="replace").splitlines()
    lo, hi = sorted((hit["start"], hit["end"]))
    a, b = max(1, lo - CONTEXT), min(len(text), hi + CONTEXT, lo + SHOWN)
    return "\n".join(f"{n:5} {text[n - 1]}" for n in range(a, b + 1))


def grade(query: str, items: list[dict], corpus: Path, model: str, effort: str) -> list[dict]:
    blocks = []
    for i, it in enumerate(items, 1):
        code = lines(corpus, it["hit"])
        blocks.append(f"Hit {i}: {it['hit']['path']}:{it['hit']['start']}-{it['hit']['end']}\nLine: {it['why']}\n"
                      + (f"Code:\n{code}" if code else "Code: (no such file)"))
    prompt = PROMPT.replace("{query}", query).replace("{hits}", "\n\n".join(blocks))
    cwd = Path(agent.tempfile.mkdtemp(prefix="bench-why-", dir="/tmp"))
    try:
        events = agent.claude(["--model", model, "--effort", effort, "--setting-sources", "", "--disable-slash-commands",
                               "--strict-mcp-config", "--tools", "", "--max-turns", "1", prompt], cwd)
    finally:
        agent.shutil.rmtree(Path.home() / ".claude/projects" / str(cwd).replace("/", "-").replace(".", "-"), ignore_errors=True)
        agent.shutil.rmtree(cwd, ignore_errors=True)
    text = next(e for e in events if e.get("type") == "result").get("result") or ""
    got = {}
    for l in text.splitlines():
        l = l.strip().strip("`")
        if l.startswith("{"):
            try:
                g = json.loads(l)
                got[int(g["hit"])] = g
            except (ValueError, KeyError):
                pass
    return [it | {"grade": got.get(i)} for i, it in enumerate(items, 1)]


def summarize(items: list[dict]) -> dict:
    graded = [it for it in items if it["grade"]]

    def share(sel, key, val):
        return round(sum(1 for it in sel if it["grade"].get(key) == val) / len(sel), 3) if sel else None

    out = {"hits": len(items), "graded": len(graded)}
    for name, sel in (("all", graded), ("correct", [it for it in graded if it["correct"]]),
                      ("wrong", [it for it in graded if not it["correct"]])):
        out[name] = {"n": len(sel), "true": share(sel, "true", "yes"), "partly": share(sel, "true", "partly"),
                     "false": share(sel, "true", "no"), "says_why": share(sel, "why", "yes"),
                     "true_and_why": round(sum(1 for it in sel if it["grade"].get("true") == "yes" and it["grade"].get("why") == "yes") / len(sel), 3) if sel else None}
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("results", nargs="+", type=Path)
    ap.add_argument("--corpus", type=Path, default=run.PROJECTS, help="the checkouts the runs searched")
    ap.add_argument("--model", default="claude-opus-5-5")
    ap.add_argument("--effort", default="medium")
    ap.add_argument("--parallel", type=int, default=4)
    ap.add_argument("--only", help="comma-separated query ids")
    args = ap.parse_args()
    queries = {q["id"]: q["text"] for q in run.queries()}
    out = {"kind": "why", "when": datetime.now().isoformat(timespec="seconds"), "grader": f"{args.model} effort {args.effort}",
           "teacher_restricted": "Anthropic Consumer Terms: evaluation only, never training data",
           "corpus": str(args.corpus), "models": []}
    for f in args.results:
        result = json.loads(f.read_text())
        for m in result["models"]:
            by_q: dict[str, list[dict]] = {}
            for row in m["rows"]:
                if args.only and row["id"] not in args.only.split(","):
                    continue
                for hit, mark in zip(row["hits"], row["score_lenient"]["marks"]):
                    by_q.setdefault(row["id"], []).append({"run": row["run"], "hit": hit, "why": hit["why"],
                                                           "correct": mark is not None})
            jobs = list(by_q.items())
            with ThreadPoolExecutor(args.parallel) as pool:
                graded = list(pool.map(lambda j: grade(queries[j[0]], j[1], args.corpus, args.model, args.effort), jobs))
            items = [it | {"id": q} for (q, _), g in zip(jobs, graded) for it in g]
            s = summarize(items)
            print(f"{f.name} {m['model']}: {json.dumps(s)}", file=sys.stderr)
            out["models"].append({"result": f.name, "model": m["model"], "summary": s, "items": items})
    print(run.save("why", out))


if __name__ == "__main__":
    main()
