# Plan: hard fork of hister

Written 2026-10-05. The decision is in [decisions.md](decisions.md). Steps 0 and 1 are done. The fork is `~/Projects/hister` (`origin` is AlexanderReaper7/hister, `upstream` is asciimoo/hister), on `master` at `2ff95bb2` with step 1 as uncommitted changes.

Scope: filters in vector search, and dates in the embedded text of local files. Questions marked **Q** are the user's to answer before that step starts.

## Found while reading `2ff95bb2`

These change the plan, so they come first.

- **Every document already embeds a metadata chunk.** `documentEmbeddingInputsWithLimit` (`server/vectorstore/embedder.go:470`) makes chunk 0 from title, URL, type, language, author, description and keywords (`documentEmbeddingContext`, `server/indexer/indexer.go:1044`), and starts every body chunk with a header of some of those fields. This was already in v0.20.0, so the prototype has it. semsearch's history import also puts title and URL into the text, so each history entry embeds them twice. A date can go into the same header (step 3), which needs no change to `Text`.
- **Keyword search already has a date range.** `Query.DateFrom` and `DateTo` (`indexer.go:96`) filter on Bleve's `updated` field. The vector search ignores them, the same as it ignores `type:`, `domain:`, `label:` and upstream's new `site:` and `has:`.
- **The vector store is brute force.** sqlite-vec v0.1.6 (bundled in `server/vectorstore/sqlitevec`) has no ANN index. A KNN query scans every vector in the user's partition, inside vec0's own loop. The same scan done as SQL, with `vec_distance_cosine` per row, is 90 times slower (step 1).
- **Postgres is a second backend** (`server/vectorstore/postgres.go`, pgvector with HNSW). Nothing here uses it.

## Step 0: build and run the fork (done 2026-10-05)

- `nix build .#hister` in the fork builds. Its `checkPhase` runs no tests: `nix/package.nix` has `subPackages = [ "." ]`, and the root package has no test files. The suite runs in `nix develop` with `go test ./...`. `server` and `server/static` only compile once the frontend is in `server/static/app` (gitignored): `nix build path:.#hister.frontend`, then copy its output there.
- The prototype data is copied to `~/.local/share/semantic-search.v0.20.0-backup` (2.7 GB) before anything opens it with the fork.
- A fork instance on a copy of the web data, port 4533, starts and serves. It logs "The semantic search embedding configuration differs from the indexed configuration". That is harmless: `EmbeddingFingerprint` (`config/config.go:215`) includes the endpoint URL, the web backlog was embedded through `:5003`, and the warning goes away with that endpoint set. Same model, different server.
- The CPU-tuned build moved to the nixcfg packaging step. The arch is `config.mine.cpu.arch` in nixcfg (`znver5`), and the fork's own flake cannot see it. There it becomes `GOAMD64=v4` and `-march=znver5 -O3` in `CGO_CFLAGS` for the bundled sqlite-vec C.

Two findings that block live checks until handled:

- **The web data has 65,233 queued embeddings.** They were queued 18:01-18:31 on 2026-09-26 by the dated history import, and the `:5003` GPU server stopped before they ran. Any instance that opens this data, upstream or fork, starts on them at once with `max_embedding_concurrency: 2` on the CPU embedder. The code data has 1,688 queued, files none. Step 4 re-embeds everything anyway, so these can wait for the GPU.
- **Query embedding is slow right now and the fork gives it 2 s.** Upstream #801 (in `master`) adds `query_embedding_timeout`, default 2 s. On 2026-10-05, 9 minutes after a boot, with load 15, a 32 GB qemu VM busy and `llama-server` at nice 12, a 5-token query took 2.5 s on `:5002`, against 0.09 s on 2026-09-26. Every semantic query failed. Not investigated further, because the cause is machine load and not fork code. Whether 2 s is the right limit for a CPU embedder sharing the machine: **Q6**.

**Q6.** `query_embedding_timeout`: keep upstream's 2 s, raise it (say 10 s), or turn it off? A failed query embedding returns keyword results only, without saying so in the CLI. Recommendation: raise it to 10 s in our configs and make semsearch report when `semantic_hits` is missing. **Answered 2026-10-05:** investigate the embedder before choosing a number.

Investigated 2026-10-05, 19:20-19:27:

- While the `stealth-bench` VM runs, the host's `system.slice` and `user.slice` are limited to CPUs `8-11,20-23` (4 cores and their SMT siblings), and `machine.slice` keeps `0-23`. The VM's emulator threads are pinned to CPUs 8 and 20, inside the host's set. `llama-server` starts with `n_threads = 12` on those 8 CPUs, at nice 12 from `system76-scheduler` (the unit says `Nice=0`).
- From 19:20 to 19:25 the service took 2.4-2.9 s per 5-token query, at load averages from 15 down to 2.3. At 19:26:47 it took 0.076 s with nothing changed. The VM had booted with the host at 19:11, so the slow window was its first quarter hour. That fits, but it is not proven.
- Ruled out by a second `llama-server` from the same store path on `:5004`, on the same CPUs: 12 threads at nice 0 gave 0.10-0.15 s, 12 threads at nice 12 gave 0.08 s, and 4 or 8 threads gave 0.08 s. Neither the thread count nor the nice value makes it slow by itself.

So the query embedding time depends on what else runs on the host's 8 CPUs, and a 2 s deadline turns a busy stretch into keyword-only search with no warning. The recommendation stands: 10 s in our configs, and semsearch warns when `semantic_hits` is missing.

**Q6 answered 2026-10-05**, after the investigation: warn when a search takes longer than 3 s, give up at 30 s, and show whatever was found by then, so a degraded search still returns something. How that maps onto the parts:

- `query_embedding_timeout: 30` in the three prototype configs.
- semsearch times each source's request and warns on stderr past 3 s.
- The query embedding is one call, so there are no partial semantic results to show. What exists by then is hister's keyword results, which semsearch ignores today. When a source returns no semantic hits because embedding failed or timed out, semsearch shows that source's keyword hits, marked as keyword-only. With `-s all`, every source that answered is shown.
- For semsearch to tell "embedding failed" from "nothing similar", the fork's search response says which one happened.

## Step 1: filters in vector search (built 2026-10-05, uncommitted)

Goal: every filter that narrows keyword results narrows semantic hits the same way, before ranking, so `--since 2w` returns the best matches from the last two weeks and not the last-two-weeks part of the top 100.

Two ways to get there:

- **A. Ask Bleve for the allowed documents, then rank only those.** Split the parsed query into its filter part and its text part, run the filter part through Bleve for matching doc IDs, and restrict the vector search to their chunks. One filter implementation for both searches, so `site:`, `has:` and anything upstream adds later work for free. The cost is a large ID set: `type:web` alone is about 83,000 IDs.
- **B. Copy the filter fields into the vec0 table** as metadata columns (`type`, `domain`, `label`, `updated`) and add `WHERE` clauses to the KNN query. Every filter is written twice, in Bleve and in SQL, and `site:` (a hostname regexp) and `has:` do not translate. A label edit has to update the vector rows too.

Measured 2026-10-05 on a copy of the web instance's `vectors.sqlite3` (105,232 chunks, 83,461 documents, 2,560 dimensions), k = 400, best of 3, query "rust async runtime comparison". The allowed set was random documents in a temp table. Scratch program in `/tmp/vecbench/src`, not committed.

| query | 83,461 allowed | 10,000 allowed | 1,000 allowed |
|---|---|---|---|
| vec0 KNN, no filter (today) | 305 ms | | |
| `vec_distance_cosine` over every chunk | 27,870 ms | | |
| `vec_distance_cosine` over allowed chunks (join) | 28,460 ms | 3,723 ms | 554 ms |
| vec0 KNN with `AND chunk_key IN (allowed chunks)` | 539 ms | 199 ms | 122 ms |

Ranking in SQL with `vec_distance_cosine` is out: 28 s. vec0's KNN accepts an `IN` constraint on its primary key, and it filters exactly. For allowed sets of 50, 1,000 and 10,000 documents, on two queries, it returned the same chunks in the same order as the brute-force join, and none outside the set. With 50 allowed documents it returned all 60 of their chunks although k was 400, so the filter runs before ranking.

Chosen: A, with the restriction done as KNN plus `chunk_key IN`, the IDs passed as one JSON parameter through `json_each`. When the filter matches every document, the restriction is skipped.

Built 2026-10-05:

- `querybuilder.SplitSemantic` (`server/indexer/querybuilder/search.go`) splits a query into the text to embed and the filters. The rule: every token `isFieldSpecific` accepts, negated or not, is a filter, and so is an alternation made only of such tokens. A negated plain word such as `-foo` stays in the text, as before. This also fixes an upstream bug: `type:web rust` used to embed the words "type:web rust".
- `Indexer.semanticAllowedDocuments` (`server/indexer/indexer.go`) runs the filters through `BuildValidated`, joins the API's `date_from`/`date_to` and the user scoping, the same restrictions the keyword search applies, and returns the matching Bleve IDs. Bleve IDs are `Document.ID()`, the vector store's `doc_id`. If the filter matches every document it returns nil and the vector search runs unrestricted.
- `VectorStore.Search` takes `allowed []string`. nil means no restriction, empty means nothing. SQLite adds `AND e.chunk_key IN (SELECT ... WHERE doc_id IN (SELECT value FROM json_each(?)))` to the KNN query. Postgres returns `ErrFilterUnsupported` for a non-nil `allowed` (Q1).
- Q2: `processWeb` keeps a submitted `Updated` and sets now only when it is 0. `applySubmissionTimestamps` keeps the earliest `Added` of the stored and submitted values, so a history import moves a captured page's `added` back to its first visit without a delete. The bookmark importers (readeck, shaarli, wallabag) already sent `Updated`, and `processWeb` used to overwrite it, so they now keep their dates too. The extension sends no timestamps and behaves as before.

Tests, in `server/indexer/semantic_filter_test.go` and `querybuilder/split_test.go`. Two documents: alpha.example ranks first for the query, beta.example second, with `result_limit: 1`. Unfiltered search returns alpha. Search with a filter only beta passes must return beta: upstream returns alpha, and a filter applied after ranking returns nothing. Covered filters: `domain:`, `site:`, `-domain:`, `updated:>=`, an alternation of domains, and the API's `date_from`. There are also tests for a filter matching nothing, for the embedded query text not containing the filter, and for the timestamp rules. Checked by mutation: with `allowed` replaced by nil, the three filter tests fail. With upstream's timestamp rules restored, the timestamp test and the `date_from` test fail. Full suite in `nix develop`: 38 packages pass, none fail.

Live, 2026-10-05, the step 1 build on a copy of the web data (its 65,219 queued jobs deleted, in the copy only), query "rust async runtime comparison", `result_limit` 100, `search_duration` of the third of three runs:

| query | time | semantic hits |
|---|---|---|
| unfiltered | 0.58 s | 100, 9 of them on github.com |
| `type:web` (matches every document, restriction skipped) | 0.72 s | 100, the same |
| `domain:github.com` | 0.41 s | 10, all github.com |
| `site:reddit.com` | 0.44 s | 4, all reddit.com |
| `-domain:github.com` (allows 99% of documents) | 1.07 s | 93, none on github.com |
| `domain:nowhere.invalid` | | none |

Query embedding was 0.11-0.18 s of each. The skipped case costs 0.14 s because Bleve still lists all 83,000 IDs before the count shows they are everything. The worst case, a filter that keeps nearly everything, adds 0.5 s. Not optimised: a count query first would save the skipped case's 0.14 s, and a negated filter could send the small excluded set as `NOT IN`. Worth it only if 1 s is felt.

`updated:>=2026-09-20` and `date_from` return the unfiltered result on this data, which is correct for it: the data was imported under upstream's rules, so every document's Bleve `updated` is its add time on 2026-09-26, and the filter matches everything. The real dates are in `metadata.updated` until the step 4 re-import.

**Q1.** Postgres: (a) delete the backend, (b) leave it unfiltered and return an error when a filter is given, or (c) implement it too. Recommendation: b. Nothing here uses Postgres, and deleting it makes every upstream cherry-pick touching it a conflict. **Answered 2026-10-05:** b, leave it, and return an error when a filter reaches it.

**Q2.** Which date does a date range filter? Bleve's `updated` for a non-local document is the time of the add (`processWeb`), not the last visit. The prototype works around that with `metadata.updated`. Options: (a) filter on `metadata.updated` when set, else `updated`, or (b) let `/api/add` set `added` and `updated` from the request, so they mean first and last time for every source, and drop the `metadata.updated` workaround in semsearch. Recommendation: b. It is a data model change, which is why it is a question. It also removes the delete-and-re-add semsearch does to correct `added`. **Answered 2026-10-05:** b, `/api/add` sets `added` and `updated` from the request.

**Q3.** With filters working, do the three instances become one, with source as `type:` or `label:`? One instance means one process, one web UI and one MCP server, and the cross-source query becomes a single search with no merge in semsearch. The Firefox extension then posts to the same instance the code indexer writes to. Recommendation: merge, but as its own step after step 1 is measured, since it means a full re-embed or a vector copy between databases. **Answered 2026-10-05:** decide after step 1, on measured filter numbers.

## Step 2: semsearch follows

- Pass `--since`/`--before` to hister as `date_from`/`date_to`, and delete the post-search filter.
- Lower `result_limit` from 100 back to about 30, since it was raised only to feed the post-search filter.
- Drop the title and URL from history entries' text, which the metadata chunk already carries, if step 3 moves the date line too.

## Step 3: dates in file text

Goal: a local file's embedded text names its date, so "the notes I wrote last March" can match.

Put the date in `DocumentContext` and let `documentEmbeddingInputsWithLimit` write it into the metadata chunk and the body header, spelled out the way semsearch does it now ("Tuesday 22 September 2026, 14:03"). `d.Text` stays unchanged, so keyword search and result previews do not show it.

**Q4.** Files only, or every document? Done for every document, the same field also gives extension-captured pages their date back (the second gap in the dates decision), and semsearch can stop writing its own date lines for code and history. Recommendation: every document. It is more than the chosen scope, which is why it is a question. **Answered 2026-10-05:** every document.

**Q5.** Which date, and does a date change re-embed? `embeddingTextChanged` (`indexer.go:288`) compares only `Text`. A last-visit date in the header changes on every visit, so re-embedding on a date change would re-embed a page on every revisit. Options: (a) embed the first date (`added`) only, which never changes, (b) embed the last date and re-embed only when the day changes, or (c) embed the last date and never re-embed for it, letting it go stale. Recommendation: a for web pages, and the modification date for files and code, where a change of date comes with a change of text anyway. **Answered 2026-10-05:** first visit for web pages, modification date for files and code.

## Step 4: one re-index, then verify live

All changes to embedded text land before this, so everything is embedded once. Run it on the GPU server on `:5003` while no chat model is loaded (16 minutes for code on 2026-09-26 figures, more for 92,000 history entries).

Live checks, each one able to fail:

- A `--since` query whose best match is older than the range returns a match inside the range, and none outside it.
- `type:` or `label:` with a query whose best overall match is from another source returns only the filtered source.
- A query naming a weekday or month ranks a file from that day above one from another.
- A page captured by the extension shows its first visit and last visit, and so does a history-only entry.

Then commit, and update the agent skill (`~/Projects/agents/agents/skills/semantic-search/SKILL.md`), which describes the post-search filter and its 100-hit limit.

## Not in this plan

- Two embedding endpoints. Check what #801 gives first.
- Re-embed throttle for pages whose text changes on every load.
- nixcfg packaging and services, after step 4.
