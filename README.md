# Semantic-Search

Local semantic search over the user's code, files and Firefox history. Nothing leaves the machine: embeddings come from the llama.cpp server that nixcfg runs.

Each source is searched from where it is used. VS Code searches code, Firefox searches web history, and file search has its own entry point. Agents reach all three through a skill. A single query across every source exists but is the rare case.

Nothing is built yet. Whether this wraps [hister](https://github.com/asciimoo/hister) or replaces it is the open question in [docs/decisions.md](docs/decisions.md), along with every decision so far and the measurements behind them.

## Where the pieces live

| piece | location |
|---|---|
| indexers, chunkers, query clients | this repository |
| embedding server, services, packaging | nixcfg, which takes this repository as a flake input |
| agent skill | the agents repository, `agents/skills/semantic-search/` |
