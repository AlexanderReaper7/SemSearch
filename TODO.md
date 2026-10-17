# TODO

- Rewrite `vscode/README.md` when the modal replaces the quick pick (docs/decisions.md, 2026-10-10).
- Fix the `index-code` help text in `src/main.rs`, which says roots are repositories or contain them one level down; a directory inside a repository now stands for it. Wait until the user's uncommitted changes to that file are settled.
- Answer the desktop window's three CEF questions, left open by its thread on 2026-10-11 (docs/decisions.md, 2026-10-10):
  - Keep `dcheck_always_on` in the CEF build? The build isn't an official one, so DCHECKs are on and any broken invariant kills the app; turning them off is a rebuild of about 3.5 h. That thread recommended keeping them until the window has run on COSMIC for a while.
  - Where does `CEF_PATH` live: the flake's dev shell, or `[env]` in `app/.cargo/config.toml`? Without it cargo silently downloads stock CEF, which falls back to X11. That thread recommended `app/.cargo/config.toml`.
  - Single instance is broken under CEF. A second launch on the shared profile directory trips CEF's ProcessSingleton, `cef::initialize` fails and the app panics with `WebviewRuntimeNotInstalled`, so the COSMIC shortcut can't toggle the open window. How should a second run reach the first?
