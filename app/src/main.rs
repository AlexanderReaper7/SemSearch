//! `semsearch-gui`: the desktop search window. A borderless window with the
//! search pill, over every source by default (docs/decisions.md, 2026-10-10).
//!
//! Wayland lets an app neither grab a global key nor place its window, so a
//! COSMIC custom shortcut runs this binary. A second run toggles the window
//! of the first instead of opening another. Escape, or the window losing
//! focus, hides it; the process stays, so the next show is instant.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use semsearch::{Source, hister};
use tauri::{Manager, WebviewWindow, WindowEvent};
use tauri_plugin_opener::OpenerExt;

/// Hits are cut to this many, as the extension's quick pick was.
const LIMIT: usize = 50;

fn source(name: &str) -> Result<Option<Source>, String> {
    match name {
        "all" => Ok(None),
        "web" => Ok(Some(Source::Web)),
        "files" => Ok(Some(Source::Files)),
        "code" => Ok(Some(Source::Code)),
        other => Err(format!("unknown source {other}")),
    }
}

/// One search, off the main thread: the library blocks on HTTP.
#[tauri::command]
async fn search(query: String, source: String) -> Result<hister::Found, String> {
    let source = self::source(&source)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut found = hister::search(source, &hister::Search { query: &query, since: None, before: None })?;
        found.hits.truncate(LIMIT);
        anyhow::Ok(found)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("{e:#}"))
}

/// A hit in its default handler: a code hit's `vscode://file` URL opens in
/// VS Code at its line, a web hit in the browser, a file in its app.
#[tauri::command]
fn open(app: tauri::AppHandle, url: String) -> Result<(), String> {
    app.opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

fn toggle(window: &WebviewWindow) {
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri_runtime_cef::cef_entry_point]
fn main() {
    let debug = std::env::var_os("SEMSEARCH_GUI_DEVTOOLS").is_some();
    tauri::Builder::default()
        // With SEMSEARCH_GUI_DEVTOOLS set, Chromium switches on the command
        // line apply too, such as --enable-logging=stderr --vmodule=...
        .runtime(tauri_runtime_cef::Cef::default().allow_chromium_command_line_args(debug))
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                toggle(&window);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![search, open])
        .setup(move |app| {
            // SEMSEARCH_GUI_DEVTOOLS=1 opens Chromium's DevTools beside the
            // window, the only way to see the page's console and styles.
            if debug
                && let Some(window) = app.get_webview_window("main")
            {
                window.open_devtools();
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Focused(false) = event {
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("running the search window");
}
