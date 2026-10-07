# Benchmarks

Code search only. Web history and files have no queries yet.

They run against a seeded hister of their own, never the real one (the user, 2026-10-06). The real index changes under a benchmark (an embedding backlog, edits, these files themselves: `queries.toml` holds every query verbatim and came first for 16 of 26 queries once indexed), so its results could not be compared or repeated.

```sh
uv run bench/run.py seed           # clone the pinned corpus and index it, once
uv run bench/run.py check          # every answer still resolves, and is embedded
uv run bench/run.py quality --note "what was different"
uv run bench/run.py latency --note "..."
uv run bench/run.py throughput --target zbox    # or local, reaperboi's card
```

## The seeded instance

`corpus.toml` pins the six repositories that hold answers, each to a commit. `seed` clones them from `~/Projects` with `git clone --shared` into `~/.local/share/semantic-search/bench/corpus`, checks out the pins, removes Semantic-Search's `bench/`, and indexes them with the installed semsearch into a hister on `:4440` with its own data directory and semsearch state (under `bench/`). Moving a pin and running `seed` again re-adds only the files that changed. Worktrees and the other repositories are left out (the user's choice), so there are fewer distractors than in real use and the scores are higher than real use would see.

Seeding embeds on reaperboi's card through `/upstream/`, about 3,700 tok/s against the zbox's 400, and evicts what the card held. Queries embed the normal way, through the failover to the zbox; the two cards' vectors agree to cosine 0.9998. hister stores the seeding endpoint in its embedding fingerprint, so this instance warns at every start that the configuration differs. That warning is expected here.

`quality` and `latency` start the instance, run, and stop it by its PID. Its log is `bench/hister.log` in the data directory. The instance shares InferMux's embedder and reranker with the real one, so the real instance's backlog is recorded with every latency and throughput result.

Each run writes `results/<time>-<kind>.json` with what it depends on (the pins, the hister and semsearch builds, a hash of the instance's config and of `queries.toml`, the real instance's embedding backlog) and prints a summary. Compare runs query by query, not only by the totals: with 26 queries one query moves a rate by 3.8 points.

## Quality

`queries.toml` holds questions about the user's code with known answers. The relevance rule is at its top: a correct piece implements the behavior, is a test that demonstrates it, or explicitly explains it. Each answer carries its `kind`, so the summary is given twice, for any correct piece and for implementation only.

An answer is found by its `anchor`, the text of its line, so it follows the code when lines move. `check` fails when an anchor is gone. A piece is correct if its span holds the answer's line. Worktrees of a repository (`<repo>-*`) would count too, but the corpus has none.

One search per query gives four orders: `keyword` (hister's first page of keyword hits), `similarity` (the 50 semantic hits), `reranked` (the 30 the reranker ordered) and `final`, semsearch's order: the reranked hits, then the semantic hits they left out. hit@k counts the queries whose first correct piece is within the top k. MRR is over the returned lists, so a miss beyond them counts 0.

The queries were written by the agent and reviewed by Codex on 2026-10-06, then reworded where they repeated the answer's comment. They were tuned against this configuration only. A held-out set, written without reading the answers' comments, does not exist yet.

## Latency

One pass over the queries in a shuffled order (`--seed`). Each query runs once, because the embedder's prompt cache would answer a repeat. The stages come from InferMux's record of hister's own requests (`/warden/requests`, InferMux 0014), matched by start time:

- `embed`: the query embedding, as the zbox's InferMux timed it. The hop from reaperboi over the tailnet is not in it but in `rest`.
- `rerank`: the rerank request.
- `rest`: total minus both, which is hister itself (the keyword search, the filtered vector search) and HTTP.

A query whose reranker was not loaded beforehand is reported apart as cold. A search with a semantic or rerank error is left out of the summary and counted.

## Throughput

Embedding endpoint throughput, batch 8, concurrency 2, which is what hister sends. It is not indexing throughput: file discovery, splitting and hister's index writes are not in it. The sample is real chunks from the vector store, rebuilt into the text hister embeds (`document:` before a metadata chunk, the title, date and language lines and `content:` before a body chunk), chosen by `--seed` from the chunks ordered by key. The sample's hash is saved; it changes when the index does. Metadata and body chunks are also timed apart.

`--target local` goes through `/upstream/`, past the failover, so it loads the model on reaperboi's card and evicts what was there.

## Agent model speed

`llm_speed.py` runs llama-bench (InferMux's llama.cpp build) on GGUF files: prompt processing (pp512) and generation (tg128) at context depths 0, 8192 and 16384, with the peak VRAM nvidia-smi attributes to the process. It runs outside InferMux, whose warden unloads its models while a foreign process uses the card.

```sh
uv run bench/llm_speed.py /srv/models/agent-candidates/*.gguf --note "..."
```

The user's minimum is 100 tok/s of generation (2026-10-06); it is read at depth 16384, since the agent's context fills with tool results. The first test of each model (depth 0) runs right after the load and is noisy; the deeper ones are not. The desktop holds about 3.4 GB of the card, which nvidia-smi does not list as compute processes, so about 6.5 GB is left for models.

## Agent

`agent.py` runs the deep search agent on the queries: a llama-server per GGUF file (or `--endpoint` for a server already running), the instant results up front, read-only tools (search, grep, read, git), at most 12 model calls. It first unloads InferMux's models, since a server that cannot allocate never shows the GPU load that makes the warden yield; the first search loads the reranker back. Searches without rerank scores are counted as degraded; a run with many is not comparable.

```sh
uv run bench/agent.py /srv/models/agent-candidates/Qwen3.5-4B-Q4_K_M.gguf --runs 3 --think \
  --max-tokens 1280 --server-args "--reasoning on --reasoning-budget 256" --note "..."
uv run bench/agent.py --rescore results/*-agent.json    # score saved answers again
```

`--claude opus` runs the agent as `claude -p` sessions instead, through the subscription: Claude Code's loop with the harness's system prompt, no built-in tools, the four tools from `mcp_tools.py` (an MCP server over the same `call()`), no user settings, hooks, plugins, skills or CLAUDE.md, an empty temp directory, and none of the calling process's `CLAUDE*` variables. A session with any other tool list stops the run. Claude Code still adds its environment block, the date and the account's email. Its results carry `teacher_restricted`: they are evaluation only, never training data.

The agent answers `path:start-end | why`, one hit per line. A hit is correct when its file is an answer's file and its range holds the answer's line and spans under 80 lines. Each run is scored twice. Strict takes the format as written. Lenient (proposed 2026-10-07) ignores bullets, bold, backticks and a `path:` label, and reads a bare `path:N` at a piece's first line as that piece, as the instant results show it; a file, a line and a why are still required.
