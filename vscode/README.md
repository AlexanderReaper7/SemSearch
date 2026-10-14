# SemSearch for VS Code

Searches code by meaning instead of by exact text. The extension runs the `semsearch` CLI, which queries the local hister instance, and opens a hit at its line and column.

## Commands

| command | key | searches |
|---|---|---|
| SemSearch: Code | Ctrl+Alt+F | code in the open folders, not their worktrees; all indexed code when no folder is open |
| SemSearch: Everything | | code, local files and Firefox history |

Both open a quick pick. A search starts from the second letter, 300 ms after the last key, and Enter starts it at once. The list keeps semsearch's order and shows one item per file, at its best piece. A code hit opens in the editor. A file or web hit opens in its default handler. A slow search, or a fallback to keyword hits or to unreranked order, is reported in the quick pick's title.

## Settings

| setting | default | effect |
|---|---|---|
| `semsearch.binary` | `""` | path to `semsearch`, such as a dev build at `~/Projects/SemSearch/target/release/semsearch`; empty uses `semsearch` on PATH |
| `semsearch.watch` | `true` | re-index an open folder's code 10 s after its last file change; read at startup |

## Requirements

- `semsearch` on PATH, or at the path in `semsearch.binary`. nixcfg installs it from this repository's main branch.
- hister listening at `SEMSEARCH_URL`, default `http://127.0.0.1:4433`, as VS Code's environment sets it. On strix, nixcfg runs it as the `hister.service` user unit (`home/alexander/features/semsearch.nix`).
- Indexed code. `semsearch-index-code.timer` indexes every repository under `~/Projects` every 15 minutes. With `semsearch.watch` on, `semsearch index-code <folder>` runs for each open folder after a change, and its output goes to the SemSearch output channel.

## Installation

The extension is a symlink to this directory:

```sh
ln -s ~/Projects/SemSearch/vscode ~/.vscode/extensions/local.semsearch-0.1.0
```

Then run "Developer: Reload Window". VS Code takes the version from `package.json`, not from the link's name, so the link keeps its name across version bumps and a reload picks up the new version.

## Development

```sh
node --test vscode/test/*.test.js
```

The tests run `extension.js` against a fake `vscode` module and a fake `semsearch` (`test/fake-semsearch.js`).

`icon.png` is `icon/favicon.svg` with its color fixed to the dark theme's `#e6edf3`, since a PNG cannot follow the theme and `currentColor` without a color renders black. It was rendered on 2026-10-14 from the one-color `favicon.svg` in the user's working tree, which was not committed then. The committed `favicon.svg` was the older version with a black lens.

The render command:

```sh
sed 's|<style>.*</style>|<style>svg{color:#e6edf3}</style>|' icon/favicon.svg > /tmp/semsearch-icon.svg
inkscape /tmp/semsearch-icon.svg -w 256 -h 256 -o vscode/icon.png
```
