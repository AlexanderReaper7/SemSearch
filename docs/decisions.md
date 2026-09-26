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

## Open - Build on hister or rewrite

hister (Go, AGPL-3.0) covers web history and files: a Firefox extension that captures full page content, a keyword index, file parsers, MCP, a web UI and a TUI. It calls an OpenAI-compatible `/v1/embeddings` endpoint, so it can use `:5002`.

What these decisions need that hister does not do, read from its source at `25dccad`:

- Semantic search ignores `type:`, `domain:` and `label:` filters. The vector query filters only on `user_id` (`server/vectorstore/sqlite.go`, `searchUser`), and the hits merge into results unfiltered. Fixes: one hister instance per source, or a patch to the vector store.
- Chunking is built into the server (`ChunkAndEmbed`) with no plugin point. Code can still be chunked outside: text under the size limit passes through as a single chunk, so one document per function works through `/api/add`.
- History import crawls, see above. Fix: our own importer posting URL and title through the API.
- One embedding endpoint. GPU-first initial indexing needs either a restart pointed at a GPU server or a proxy in front.

Any code copied from hister carries AGPL-3.0 with it. Reading it for ideas does not.

## Future - Phone

Search from the OnePlus 8 Pro (Snapdragon 865, 12 GB). Not measured. Octen-4B Q8 needs 4.3 GB for weights, and an estimated 10-20 tok/s would make queries workable and on-phone indexing impractical. Either the phone queries the desktop over Tailscale, or it gets a smaller model, which means re-embedding everything with that model. Deferred 2026-09-26.
