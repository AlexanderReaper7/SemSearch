# Decisions

Decisions and their reasoning, dated. Open questions stay listed, since a dropped question silently becomes a wrong assumption.

## 2026-09-26 - Fully local

No hosted embedding API and no cloud index. This rules out mgrep, which keeps its index in the Mixedbread cloud and needs `mgrep login`.

## 2026-09-26 - Sources are searched separately

Code is searched from VS Code, web history from Firefox, files from a file search. Searching everything at once must be possible but will be rare, so the design optimises for one source per query.

Consequence: every source uses the same embedding model. A cross-source query then sends one query vector to each source and merges by similarity, which only works when the scores come from the same model.

## 2026-09-26 - One embedding model, Octen-Embedding-4B Q8_0

The model nixcfg already serves on `:5002` (`modules/nixos/llm.nix`), CPU-only. It stays resident for queries and incremental updates. Initial indexing runs on the GPU whenever the GPU is free, because a backlog on the CPU takes days.

Measured 2026-09-26 on the RTX 3080 and Ryzen 9 9900X, same llama.cpp build, batches of 8 real text chunks:

| chunk size | GPU | CPU | ratio |
|---|---|---|---|
| ~95 tokens | 3,257 tok/s | 219 tok/s | 15x |
| ~520 tokens | 4,839 tok/s | 202 tok/s | 24x |
| ~1,250 tokens | 5,265 tok/s | 186 tok/s | 28x |

A short query embeds in 0.09 s on the CPU. GPU and CPU vectors for the same input have cosine similarity of 0.9997 or higher, so an index built on the GPU is searched with CPU query vectors without re-embedding.

The GPU server needs about 6.7 GB of VRAM at `--ubatch-size 2048`, `--ctx-size 8192`. `--ubatch-size 4096` does not fit next to the desktop. It cannot share the card with any chat model on `:5001`, so GPU indexing only happens while no chat model is loaded.

Volume on 2026-09-26: `~/Projects` holds 19.5 MB of tracked text, about 4.9M tokens, so about 16 minutes on the GPU against 7 hours on the CPU.

## 2026-09-26 - Web history content only when it comes free

A history entry carries page content only if the content was captured while the page was open, for example by a browser extension that was running during the visit. Old history is indexed from what Firefox already stores (URL, title, visit times) and is never crawled again.

Reason: the main profile has 100,167 visited URLs. Downloading them all again is slow, hits every site in the history, and returns today's page rather than the page that was read.

hister's `import browser-history` does the opposite: `importDB` in `cmd/browser.go` queues every URL into a crawl job. It cannot be used as is.

## 2026-09-26 - Separate repository, generic name

A separate repository because it has two consumers, VS Code and the agents, so neither nixcfg nor the agents repository owns it. The same pattern as bw-app-gate.

The name is known to be extremely generic. Kept because it says what the thing is.

## 2026-09-26 - Prototype: three unmodified hister instances

hister v0.20.0 from its upstream flake, one instance per source, configs in `prototype/`: web on `:4433`, files on `:4434`, code on `:4435`. Each keeps its data in `~/.local/share/semantic-search/<source>/`. `semsearch` (this repository, Rust) feeds the web and code instances and queries all three. The files instance indexes its configured directories on its own.

No core patch was needed. Per-source instances work around the filter gap below, and code chunking happens before hister sees the text.

Code pieces:

- `text-splitter` cuts each file into pieces of 400 to 3000 characters along the tree-sitter syntax tree (10 grammars), along headings for Markdown, and along blank lines otherwise. Below `max_context_length` hister embeds a piece as one vector.
- A piece's URL is `vscode://file/<path>:<line>:1`. It is unique per piece, and a click in hister's web UI opens VS Code at that line.
- `~/.local/share/semantic-search/code-state.json` maps each file to its hash and piece URLs. An unchanged file is skipped. A changed or removed file has its old pieces deleted, since line numbers in the URLs shift.
- Files come from a walk that respects `.gitignore`, under every git repository at or one level below the given roots.

No query or document prefix. On nixcfg's modules, no prefix and Qwen3's `Instruct: ...\nQuery: ` prefix both ranked 7 of 7 test queries correctly, with mean margins of 0.232 and 0.218.

`similarity_threshold` is 0.4, up from 0.3. In the files instance, a query whose right answer scored 0.65 had its next hit at 0.36.

History import: `semsearch import-history` copies `places.sqlite`, reads URL, title and last visit, and adds only URLs the instance lacks, so content captured by the extension is never overwritten. The main profile gave 92,266 entries after dropping fragments, `utm_` links and duplicates. A second run added 0.

The Firefox extension (AMO `hister` 0.31.0) defaults to `http://127.0.0.1:4433/`, the web instance, so it needs no configuration. Its content script only submits a visible page.

hister behaviour the client has to handle, found while building it:

- Adds return before embedding. Embedding runs from a queue persisted in the instance's database, so an interrupted run resumes.
- The queue logs `database is locked` while a batch import writes. The job is retried, not lost.
- A semantic hit that is also a keyword hit has no `document` of its own; its title is in `documents`.
- Semantic search returns `semantic_search.result_limit` hits and ignores the query's `limit`.
- `HISTER__SEMANTIC_SEARCH__EMBEDDING_ENDPOINT` overrides the endpoint without editing the config, which is how the backlog went to a GPU server on `:5003`.

## 2026-09-26 - Dates in results, in filters and in the embedded text

The user asked for date and time, and chose all three places.

Which times: web history carries first visit, last visit and visit count, read from `moz_historyvisits`. Code carries the file's first and last commit, from one `git log` per repository, or the modification time for a file with uncommitted changes. Files carry what hister's file indexer stores: first indexing and modification time.

Where they live in hister: the first time is `added`. The last time is `metadata.updated`, because hister sets `updated` to the time of the add for every document that is not a local file (`document/document.go`, `processWeb`). hister also keeps a stored `added` over a new one, so correcting it takes a delete and re-add.

Results: every hit shows its last time. `--json` adds `added`, `updated` and, for web history, `visits`. The VS Code quick pick shows the date too.

Filters: `--since` and `--before` on the last time. hister's vector search cannot filter by date, so semsearch filters the hits after the search. `result_limit` went from 30 to 100 so that a filter has more to choose from. A narrow range can still come back empty when older hits fill the 100.

Embedded text: an imported history entry embeds as its title, URL and "Visited 3 times, first on Tuesday 22 September 2026, 14:03, last on ...". A code piece starts with "Changed <date>". Weekday and month are spelled out so a query naming them has words to match. Two gaps, both left open:

- Files have no date in their embedded text. hister's directory indexer writes them, and semsearch cannot change their text without patching hister or replacing its file indexer.
- A page the extension captures loses its date line, since the extension sends the page's own text. Its dates still show in results and filters: `added` stays the first visit, and hister's `updated` is the last submission, which is the last visit.

Cost: a new date in the embedded text is a re-embed. For code that is one file per commit. For history, every import re-embeds the entries visited since the last one.

## 2026-09-26 - When hister re-embeds

hister embeds a document when it is new, or when an add carries text different from the stored text (`embeddingTextChanged` in `server/indexer/indexer.go`). Title, URL, dates and metadata do not count. The extension submits a page on load, then at most every 30 s while its text keeps changing, and again when the tab is hidden or closed. A revisit with unchanged text costs an index write. A page whose text changes on every load (a clock, a comment count, a live feed) is embedded again on every visit, and every 30 s while it stays open and changing. On the CPU embedder that is the main running cost. No throttle exists in hister; a skip rule per site, or a patch that ignores small changes, are the options if it shows up.

## 2026-10-05 - Hard fork of hister

The open question below is closed: build on hister, as a hard fork at [AlexanderReaper7/hister](https://github.com/AlexanderReaper7/hister), cloned to `~/Projects/hister`. It starts from upstream `master` at `2ff95bb2`, 47 commits past v0.20.0, and diverges freely. Upstream fixes get cherry-picked by hand when wanted.

The facts on the table when the user chose:

- Upstream moves fast: 42 commits and 125 changed files between `25dccad` and `2ff95bb2`, in about ten days.
- A thin patch fork's selling point was sending general fixes upstream. Upstream's `CONTRIBUTING.md` says "AI should never be the main author of the PR" and requires human-written issue and PR text, so patches the agent writes cannot go upstream anyway.

First scope, chosen by the user: filters in vector search, and dates in the embedded text of local files. Left out for now: two embedding endpoints (upstream #801 already gives queries their own slots) and a re-embed throttle (no measured cost yet). The plan and its open questions are in [hister-fork.md](hister-fork.md).

## 2026-10-05 - One instance for every source, code as its own document type

The user put code search first and files under `~` second, and chose:

- **One hister instance for every source**, replacing the three prototype instances. The source becomes a filter. Measured cost on 83,000 web documents: 0.4-0.5 s for a filter narrowing to one source, at most 0.5 s more for one keeping nearly everything ([hister-fork.md](hister-fork.md), step 1). Supersedes "Prototype: three unmodified hister instances" for how instances run.
- **A `code` document type in the fork**, next to `web`, `local` and `remote`. semsearch posts code pieces with it, so the sources are `type:code`, `type:local` and `type:web`. Before this, code pieces were stored as `type:web` with domain `file`, because of their `vscode://file/` URLs. Chosen over a `code` label, which hister would have had to special-case and a user can edit.
- **Dates in hister's embedded metadata for every document, before any re-index**, so code is embedded once. A web page embeds its first visit (`added`), which never changes. Code and files embed their modification date (`updated`), which only changes along with the text. semsearch stops writing its own date lines.
- **Services in nixcfg**, as systemd user units, built from the fork and this repository as flake inputs.
- **Freshness**: a systemd timer runs `index-code` for normal use, and the VS Code extension re-indexes the open project when a file in it changes.
- **Query embedding**: warn past 3 s, give up at 30 s, and show what was found by then. In practice that means keyword hits when the semantic part fails.

## 2026-10-06 - Reranking with jina-reranker-v3.5

The live test on teaterihuskvarna showed the problem: for "booking a volunteer shift that has no places left", the answer (`ShiftService.java:99`) ranked 17th by similarity, under pieces of the same file that matched only through their metadata chunk.

The user chose:

- **jina-reranker-v3.5** (2026-07-14, 0.6B, Qwen3-0.6B base, multilingual, listwise, CC BY-NC 4.0), over Qwen3-Reranker (2025, runs in llama-server today) and CLM-v0.1-8B (a verifier for agent actions, not a document reranker).
- **Reranking in the hister fork**, so the web UI, the Firefox extension's search and semsearch all get it.
- **An InferMux model on the RTX 3080, one model at a time**: a rerank evicts a loaded chat model, and a chat request evicts the reranker. Chosen over keeping it resident (chat models lose about 2.5 GB) and over raising every chat model's `-fitt`.
- **Hybrid candidates**: the best 10 keyword hits and the best semantic hits, 30 together, reranked into one order.

Serving: llama.cpp cannot run the model yet (ggml-org/llama.cpp#26286 is open, and its author does not plan llama-server support), so `rerank/server.py` serves it with transformers behind llama-server's `/v1/rerank` API. It imports the model's own `modeling.py` from the weights directory (revision `e8a93f33`, under `/srv/models/hf`), not through `trust_remote_code`. nixcfg runs it on ComfyUI's CUDA interpreter.

Measured on the RTX 3080, bf16, 2026-10-06:

| | |
|---|---|
| weights | 1.14 GB, about 3.1 GB held after the first request |
| load | 2.2 s, plus 1.4 s for the first call's kernels |
| 30 code pieces, 26-30k characters | 0.48-0.65 s |
| 30 pieces, 40k characters | 1.08 s |
| same on the CPU, fp32 | 32-75 s, so no CPU fallback |

On six queries with known answers in teaterihuskvarna, reranking the top 30 moved the answer from rank 17 to 3, from 3 to 1 twice, from 3 to 2, and kept rank 1. The Swedish query had no answer in its top 30 before or after.

In hister: `semantic_search.rerank` in the config. Results gain `reranked` (doc_id, url, rerank_score, best first) and `rerank_error`. A semantic hit is read by its best body chunk, never by its metadata chunk; a keyword hit by the start of its text; each with its title, cut at 2000 characters. Only the first page of a relevance-sorted search is reranked. Any failure leaves the results in their old order and says why.

The query embedding, not the reranker, is the slow part today: 2.7-3.0 s on the CPU embedder under load on 2026-10-06, against 0.09 s on 2026-09-26.

## Closed 2026-10-05 - Build on hister or rewrite

hister (Go, AGPL-3.0) covers web history and files: a Firefox extension that captures full page content, a keyword index, file parsers, MCP, a web UI and a TUI. It calls an OpenAI-compatible `/v1/embeddings` endpoint, so it can use `:5002`.

What these decisions need that hister does not do, read from its source at `25dccad`:

- Semantic search ignores `type:`, `domain:` and `label:` filters. The vector query filters only on `user_id` (`server/vectorstore/sqlite.go`, `searchUser`), and the hits merge into results unfiltered. Fixes: one hister instance per source, or a patch to the vector store.
- Chunking is built into the server (`ChunkAndEmbed`) with no plugin point. Code can still be chunked outside: text under the size limit passes through as a single chunk, so one document per function works through `/api/add`.
- History import crawls, see above. Fix: our own importer posting URL and title through the API.
- One embedding endpoint. GPU-first initial indexing needs either a restart pointed at a GPU server or a proxy in front.

Any code copied from hister carries AGPL-3.0 with it. Reading it for ideas does not.

## Future - Phone

Search from the OnePlus 8 Pro (Snapdragon 865, 12 GB). Not measured. Octen-4B Q8 needs 4.3 GB for weights, and an estimated 10-20 tok/s would make queries workable and on-phone indexing impractical. Either the phone queries the desktop over Tailscale, or it gets a smaller model, which means re-embedding everything with that model. Deferred 2026-09-26.
