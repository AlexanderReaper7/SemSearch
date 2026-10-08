# Benchmarks

Code search only. Web history and files have no queries yet.

They run against a seeded hister of their own, never the real one (the user, 2026-10-06). The real index changes under a benchmark (an embedding backlog, edits, these files themselves: the queries are verbatim in `queries/` and came first for 16 of 26 queries once indexed), so its results could not be compared or repeated.

```sh
uv run bench/run.py checkout       # clone the pinned corpus, without indexing
uv run bench/run.py seed           # clone the pinned corpus and index it, once
uv run bench/run.py check          # every query is well formed and its answers resolve
uv run bench/run.py quality --note "what was different"            # dev queries
uv run bench/run.py quality --split test --note "..."             # for reporting only
uv run bench/run.py latency --note "..."
uv run bench/run.py throughput --target zbox    # or local, reaperboi's card
```

## The seeded instance

`corpus.toml` pins 74 repositories, each to a commit: the user's own (six since 2026-10-06, 28 more since 2026-10-07) and 40 open-source ones chosen on 2026-10-07 across about 25 languages and kinds of software, popular and obscure. `checkout` clones a repository with a `url` blobless from it and the others from `~/Projects` with `git clone --shared`, into `~/.local/share/semantic-search/bench-v2/corpus`, and checks out the pins. Vendored third-party code, committed build output, data dumps and translations into other languages are left out by sparse checkout (`exclude`, with `keep` for the source language); `checkout` prints what each repository leaves out, in files and bytes. Semantic-Search's `bench/` is removed. After that the corpus is 142 MB in 24,550 indexable files, with 2,871 images, 65 sounds, 74 PDFs and no video. `corpus-sizes.json` has each repository's indexable size.

`seed` indexes the corpus with the installed semsearch into a hister on `:4440` with its own data directory and semsearch state (under `bench-v2/`). Moving a pin and running `seed` again re-adds only the files that changed. Worktrees are left out, so there are fewer distractors than in real use. `SEMSEARCH_BENCH` points the scripts at another instance; `bench/` holds the six-repository corpus of 2026-10-06 and its results.

Seeding embeds on reaperboi's card through `/upstream/`, about 3,700 tok/s against the zbox's 400, and evicts what the card held. Queries embed the normal way, through the failover to the zbox; the two cards' vectors agree to cosine 0.9998. hister stores the seeding endpoint in its embedding fingerprint, so this instance warns at every start that the configuration differs. That warning is expected here.

`quality` and `latency` start the instance, run, and stop it by its PID. Its log is `hister.log` in the instance's directory. The instance shares InferMux's embedder and reranker with the real one, so the real instance's backlog is recorded with every latency and throughput result.

Each run writes `results/<time>-<kind>.json` with what it depends on (the pins, the hister and semsearch builds, a hash of the instance's config and of `queries/`, the real instance's embedding backlog) and prints a summary. Compare runs query by query, not only by the totals.

## Splits

Every repository is dev or test as a whole (`split` in `corpus.toml`; docs/decisions.md, 2026-10-07): 50 dev, 24 test. Configurations, prompts and agent models are tuned on dev, where reading failures is allowed. Test is run to report a result, not to decide between configurations. A fine-tuned model may train on dev repositories and on repositories in neither split, never on test ones, so one that memorized a repository's layout shows up as a gap between the splits.

## Quality

`queries/<repo>.toml` holds the queries written for a repository, with known answers.

```toml
[[query]]
id = "shift-full"
text = "reject a volunteer booking when everyone else has already taken the available places"
category = "behavior"          # behavior, explanation, usage, symptom, config, media
adversarial = "decoy"          # optional: decoy, near-miss, negation, no-answer, short, verbose, typo, language, injection
note = "the trap"              # with adversarial
lang = "sv"                    # optional, when not English
by = "claude"                  # the writer: claude or codex
answers = [
  { repo = "teaterihuskvarna", path = "src/.../ShiftService.java", line = 109, kind = "code", anchor = "if (shift.placesLeft(...) == 0) {" },
  { repo = "godot-demo-projects", path = "2d/.../coin.wav", kind = "audio" },
]
```

Relevance rule (the user, 2026-10-06, from Codex's review): a piece is a correct hit if it implements the behavior (kind `code`), is a test that demonstrates it (`test`), or explicitly explains it (`doc`: a decision, a comment, documentation). A mention, a call that delegates the decision, or configuration next to it is not. Since 2026-10-07 a configuration value that is itself what a `config` query asks for is correct too (`config`). Queries avoid the identifiers and the comment wording of their answers.

A text answer is found by its `anchor`, the trimmed text of its line, so it follows the code when lines move; every piece whose span holds that line is correct, and worktrees (`<repo>-*`) count at the same text. A media answer (`image`, `audio`, `video`, `pdf`) is the whole file. Nothing embeds media yet, so a media query is always a miss: it stays in every total and the summary says how many there are, so the gap shows instead of disappearing. A query with `answers = []` (adversarial `no-answer`) is left out of MRR; `quality` reports its separation instead, the share of pairs where its best rerank score is below the rerank score of an answerable query's first correct hit (1.0 means a threshold could tell them apart), and `agent.py` reports how often the agent showed nothing.

One search per query gives four orders: `keyword` (hister's first page of keyword hits), `similarity` (the 50 semantic hits), `reranked` (the 30 the reranker ordered) and `final`, semsearch's order: the reranked hits, then the semantic hits they left out. hit@k counts the queries whose first correct piece is within the top k. MRR is over the returned lists, so a miss beyond them counts 0. The final order is also given per split, category, adversarial type, writer and language.

How many queries a comparison needs: between the two chunkers the per-query difference in reciprocal rank had a standard deviation of 0.34, so a paired test at 95% and 80% power needs about 90 queries to see an MRR change of 0.10, 350 for 0.05 and 1,000 for 0.03.

## Writing queries

The 26 queries of 2026-10-06 were written by the agent and reviewed by Codex, then reworded where they repeated the answer's comment; configurations were tuned on them, so their repositories are dev. The rest come from `write_queries.py` (2026-10-07):

```sh
uv run bench/write_queries.py write     # drafts by both writers: bench/drafts/<writer>/<repo>.toml
uv run bench/write_queries.py review    # each writer reviews the other's drafts
uv run bench/write_queries.py merge     # drops, deduplicates, writes queries/<repo>.toml
uv run bench/write_queries.py status
```

Two writers work on every repository without seeing each other's queries: Claude (`claude -p`, Opus 5.5, effort medium, only Read, Grep and Glob, no user settings) and Codex (`codex exec`, gpt-6-sol, effort high, read-only sandbox, web, browser and apps off). Both get the same prompt (`PROMPT` in the script) and work from the checkout alone. Per repository and writer: 4 + 3.6·√MB queries (3 to 20) of which about 15% adversarial, the types rotated across repositories, plus one no-answer from 7 queries up, and media queries for the repositories that have media (`MEDIA`). A draft is kept once every query passes `run.py`'s `problems` (well formed, anchors found, media files of the right type), after up to two rounds of fixes in the same session.

Each writer then reviews the other's draft against the same rules and the files, with a verdict per query: `keep`, `fixed: ...` or `drop: ...`. `merge` leaves out the dropped and merges duplicates: two queries that share an answer place (the same file, lines within 3) and whose texts reach Octen cosine 0.85. The kept one is the one its reviewer kept unchanged, else the one from the writer with fewer queries kept so far in that repository. The merged and dropped queries are listed in `drafts/merged.md`. The user reviews a random 50 of the queries on the user's repositories, and the error rate there decides whether more review is needed.

Codex runs on the OpenAI models that are to generate training data (docs/deep-search.md), so each query records its writer and results are broken down by it: a fine-tuned model whose gap between Codex- and Claude-written test queries is wider than its base model's has learned the writer's style. The queries are never training data.

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

## Chunking

`examples/chunk_stats.rs` cuts real files three ways and counts the bad cuts against the grammar's own parse of each file: `src/chunk.rs`, text-splitter's CodeSplitter (what chunk.rs used for ten grammars until 2026-10-07, so only those get its row), and the blank-line TextSplitter that a file with no grammar gets. The metrics are defined at the top of the file. A file whose parse runs past `PARSE_BUDGET` is counted as given up and not measured.

```sh
cargo run --release --example chunk_stats -- [--ext rs,py] PATH...
cargo run --release --example chunk_stats -- --dump FILE   # each piece, its bad cuts marked
cargo run --release --example chunk_stats -- --tree FILE   # node kinds, for LEADING
```

The bench corpus on 2026-10-07, before → after, in percent of cuts (pieces under 400 characters in percent of pieces). Before is what index-code did until then: CodeSplitter for the ten grammars it had, the blank-line splitter for PowerShell and Svelte.

| | files | needless | no blank | detached | mid-line | < 400 |
|---|---|---|---|---|---|---|
| Rust | 15 | 0.0 → 0.0 | 26.4 → 0.8 | 12.9 → 0.8 | 6.4 → 0.0 | 9.0 → 2.2 |
| Python | 172 | 0.0 → 0.0 | 19.0 → 1.8 | 8.9 → 0.1 | 0.7 → 0.0 | 7.9 → 3.4 |
| TypeScript | 221 | 0.0 → 0.2 | 16.3 → 0.2 | 9.9 → 0.0 | 13.1 → 0.0 | 20.0 → 8.9 |
| JavaScript | 18 | 0.0 → 0.0 | 46.6 → 23.0 | 15.6 → 1.1 | 11.8 → 0.0 | 21.8 → 8.3 |
| Java | 307 | 0.0 → 0.5 | 50.4 → 1.9 | 34.1 → 1.5 | 5.6 → 0.0 | 16.7 → 6.4 |
| Go | 619 | 0.0 → 0.1 | 16.7 → 2.8 | 11.5 → 0.2 | 4.6 → 0.0 | 10.3 → 3.4 |
| Nix | 91 | 0.0 → 8.5 | 65.7 → 17.6 | 33.0 → 5.5 | 16.4 → 0.0 | 41.9 → 3.9 |
| Bash | 27 | 0.0 → 8.7 | 43.7 → 3.9 | 24.8 → 1.0 | 16.0 → 0.0 | 14.2 → 5.4 |
| PowerShell | 14 | 32.1 → 0.0 | 19.1 → 23.7 | 0.0 → 1.0 | 0.0 → 0.0 | 7.6 → 3.6 |
| Svelte | 318 | 42.3 → 0.0 | 25.6 → 11.8 | 0.2 → 0.0 | 0.0 → 0.0 | 10.2 → 6.8 |

The JavaScript row leaves out the corpus's vendored `.min.js` bundles. A bundle is one line, so every cut in it is mid-line: with them, 82.8% → 67.7%. The needless cuts in Nix and Bash are inside strings of embedded shell longer than a piece, which chunk.rs cuts at lines. The detached comments left in Nix are long comments above code near 3,000 characters; the two cannot share a piece, and the code stays whole.
PowerShell's no-blank rate rises because of one file, `generate-application-map.ps1`: an array of one hashtable per line, 58 lines and 11,845 characters with no blank line, where 17 of the language's 23 such cuts fall, each between two entries.

The languages chunk.rs added, on the two to four open-source repositories per language that the per-language evaluation cloned on 2026-10-07 (not pinned). Before is the blank-line splitter, which every one of them got until then.

| | files | needless | no blank | detached | mid-line | < 400 |
|---|---|---|---|---|---|---|
| C | 2,062 | 46.6 → 2.3 | 7.2 → 9.4 | 1.3 → 1.6 | 0.0 → 0.0 | 3.2 → 2.3 |
| C++ | 4,375 | 38.6 → 0.4 | 16.4 → 20.6 | 3.5 → 3.4 | 0.0 → 0.0 | 7.4 → 3.8 |
| Objective-C | 639 | 35.1 → 0.4 | 14.9 → 7.6 | 1.2 → 0.1 | 0.0 → 0.0 | 8.6 → 3.3 |
| PHP | 4,131 | 62.7 → 0.2 | 14.0 → 10.3 | 2.7 → 1.4 | 0.0 → 0.1 | 7.0 → 4.7 |
| Ruby | 6,957 | 67.9 → 0.3 | 2.1 → 2.4 | 0.4 → 0.2 | 0.0 → 0.0 | 10.3 → 8.0 |
| Kotlin | 3,393 | 47.2 → 0.5 | 5.5 → 5.0 | 1.2 → 1.4 | 0.0 → 0.0 | 8.7 → 5.0 |
| Swift | 929 | 55.5 → 0.2 | 9.1 → 5.7 | 0.6 → 0.5 | 0.0 → 0.0 | 6.6 → 3.0 |
| Dart | 1,019 | 43.2 → 0.3 | 6.8 → 6.1 | 0.2 → 0.1 | 0.0 → 0.0 | 6.2 → 2.6 |
| Zig | 910 | 54.5 → 0.4 | 4.5 → 5.4 | 1.0 → 0.6 | 0.5 → 0.1 | 3.3 → 2.2 |
| Lua | 986 | 46.4 → 1.5 | 24.2 → 22.5 | 2.2 → 0.6 | 0.3 → 0.4 | 6.4 → 5.7 |
| Elixir | 2,192 | 59.2 → 0.1 | 0.6 → 0.7 | 0.0 → 0.0 | 0.1 → 0.0 | 4.2 → 3.5 |
| Erlang | 712 | 18.4 → 0.1 | 8.6 → 6.6 | 1.1 → 0.2 | 0.0 → 0.0 | 3.1 → 1.6 |
| Clojure | 1,861 | 25.9 → 0.0 | 13.9 → 9.6 | 0.4 → 0.1 | 0.0 → 0.0 | 6.0 → 3.2 |
| Haskell | 474 | 32.5 → 0.0 | 25.0 → 19.2 | 1.9 → 1.7 | 0.0 → 0.0 | 5.4 → 2.2 |
| OCaml | 2,131 | 27.7 → 0.2 | 21.6 → 19.0 | 0.8 → 0.2 | 0.9 → 0.2 | 11.4 → 9.7 |
| OCaml interfaces | 1,072 | 31.7 → 0.0 | 0.4 → 0.2 | 0.1 → 0.1 | 0.0 → 0.0 | 25.6 → 21.6 |
| PowerShell | 1,285 | 70.6 → 0.7 | 2.5 → 7.5 | 0.6 → 1.3 | 0.0 → 1.4 | 4.7 → 2.8 |
| Svelte | 968 | 55.2 → 0.4 | 7.3 → 4.3 | 0.2 → 0.0 | 0.0 → 0.0 | 20.2 → 17.0 |
| GDScript | 1,015 | 9.9 → 0.0 | 6.8 → 5.0 | 0.7 → 0.3 | 0.0 → 0.0 | 15.1 → 8.7 |

The grammars fail on part of the C and C++ samples (50.7% and 39.2% of files have an error node, 7.7% and 14.1% of bytes); chunk.rs still cuts those files along the nodes that did parse. No file in either run hit the parse budget.

chunk.rs alone over every code file under `~/Projects`, the cargo registry and these samples (283,030 files, 3,075 MB, dependency trees included) took 316 s, 103 ms/MB, with no crash or hang. The slowest file took 1.3 s, a 365 KB gperf-generated header in ghostty, which CodeSplitter did not finish in 600 s.
