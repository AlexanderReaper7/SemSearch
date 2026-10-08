# SemSearch

Local semantic search over the user's code, files and Firefox history. Nothing leaves the machine: embeddings come from the llama.cpp server that nixcfg runs.

Each source is searched from where it is used. VS Code searches code, Firefox searches web history, and file search has its own entry point. Agents reach all three through a skill. A single query across every source exists but is the rare case.

Everything lives in one instance of [AlexanderReaper7/hister](https://github.com/AlexanderReaper7/hister), a hard fork of [hister](https://github.com/asciimoo/hister), configured by [config/hister.yml](config/hister.yml). A source is a filter: `type:code`, `type:file` or `type:web`. The fork's plan is [docs/hister-fork.md](docs/hister-fork.md). Every decision so far, and the measurements behind them, are in [docs/decisions.md](docs/decisions.md).

nixcfg builds both from `nix/package.nix` here and in the fork, and runs `hister.service` plus a 15-minute `semsearch-index-code.timer` as user units.

```sh
cargo build --release
semsearch index-code ~/Projects                 # chunk and sync every repository
semsearch import-history <profile>/places.sqlite # URL and title, never a fetch
semsearch search -s code|web|files|all "query"  # SEMSEARCH_URL, default :4433
semsearch search -s web --since 2w "query"      # also --before; a date, date and time, or age
```

`vscode/` is a VS Code extension (Ctrl+Alt+F, "SemSearch: Code") that calls `semsearch` and opens the hit at its line. Symlink it into `~/.vscode/extensions/local.semsearch-0.1.0`.

## Where the pieces live

| piece | location |
|---|---|
| indexers, chunkers, query clients | this repository |
| embedding server, services, packaging | nixcfg, which takes this repository as a flake input |
| agent skill | the agents repository, `agents/skills/semsearch/` |
