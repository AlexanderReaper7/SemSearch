# Semantic-Search

Local semantic search over the user's code, files and Firefox history. Nothing leaves the machine: embeddings come from the llama.cpp server that nixcfg runs.

Each source is searched from where it is used. VS Code searches code, Firefox searches web history, and file search has its own entry point. Agents reach all three through a skill. A single query across every source exists but is the rare case.

A prototype runs on three unmodified [hister](https://github.com/asciimoo/hister) instances, one per source. Whether that stays or gets rewritten is the open question in [docs/decisions.md](docs/decisions.md), along with every decision so far and the measurements behind them.

```sh
cargo build --release
semsearch index-code ~/Projects                 # chunk and sync every repository
semsearch import-history <profile>/places.sqlite # URL and title, never a fetch
semsearch search -s code|web|files|all "query"
semsearch search -s web --since 2w "query"      # also --before; a date, date and time, or age
```

`vscode/` is a VS Code extension (Ctrl+Alt+F, "Semantic Search: Code") that calls `semsearch` and opens the hit at its line. Symlink it into `~/.vscode/extensions/local.semantic-search-0.1.0`.

## Where the pieces live

| piece | location |
|---|---|
| indexers, chunkers, query clients | this repository |
| embedding server, services, packaging | nixcfg, which takes this repository as a flake input |
| agent skill | the agents repository, `agents/skills/semantic-search/` |
