// Asks semsearch for hits as the user types and lists them in a quick pick,
// from the second letter, 300 ms after the last key. Code hits carry a
// `vscode://file/<path>:<line>:<column>` URL and open there; web and file hits
// open in the default handler.
//
// While a folder is open, a change to a file in it re-indexes that folder, so
// code search sees edits within seconds instead of at the next timer run.

const vscode = require('vscode');
const { execFile } = require('child_process');
const fs = require('fs');
const os = require('os');
const path = require('path');

function binary() {
  const configured = vscode.workspace.getConfiguration('semanticSearch').get('binary');
  if (configured) return configured;
  const build = path.join(os.homedir(), 'Projects/Semantic-Search/target/release/semsearch');
  return fs.existsSync(build) ? build : 'semsearch';
}

// One semsearch process: its handle, to kill it, and its outcome. A slow
// search or a fallback to keyword hits is reported on stderr, as `warning`.
// Code search covers the open folders, not their worktrees or the other
// repositories (the user, 2026-10-06); with no folder open, all code.
function search(source, query) {
  const args = ['search', '--json', '-s', source, '-n', '50'];
  if (source === 'code') for (const folder of vscode.workspace.workspaceFolders || []) args.push('--in', folder.uri.fsPath);
  args.push('--', query);
  let child;
  const done = new Promise((resolve, reject) => {
    child = execFile(binary(), args, { maxBuffer: 16 << 20 }, (err, stdout, stderr) => {
      if (err) return reject(Object.assign(new Error(stderr || err.message), { killed: err.killed }));
      resolve({ hits: stdout.split('\n').filter(Boolean).map((line) => JSON.parse(line)), warning: stderr.trim() });
    });
  });
  return { child, done };
}

// Unix seconds -> `2026-09-22 14:03` in local time.
function date(unix) {
  if (!unix) return '';
  const d = new Date(unix * 1000);
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

// vscode://file/home/x/a b.rs:12:5 -> { file: '/home/x/a b.rs', line: 12, column: 5 }
function codeLocation(url) {
  const m = /^vscode:\/\/file(\/.*):(\d+):(\d+)$/.exec(url);
  return m && { file: decodeURIComponent(m[1]), line: Number(m[2]), column: Number(m[3]) };
}

async function open(hit) {
  const loc = codeLocation(hit.url);
  if (!loc) return vscode.env.openExternal(vscode.Uri.parse(hit.url));
  const doc = await vscode.workspace.openTextDocument(loc.file);
  const pos = new vscode.Position(loc.line - 1, loc.column - 1);
  await vscode.window.showTextDocument(doc, { selection: new vscode.Range(pos, pos) });
}

const MIN_LETTERS = 2;
const DEBOUNCE_MS = 300;

function item(hit) {
  const loc = codeLocation(hit.url);
  return {
    label: loc ? `${vscode.workspace.asRelativePath(loc.file)}:${loc.line}` : hit.title || hit.url,
    description: date(hit.updated),
    detail: hit.chunk.replace(/\s+/g, ' ').slice(0, 200),
    // The list is semsearch's order for the text, not a filter of it.
    alwaysShow: true,
    hit,
  };
}

// One item per file, at its best piece; hits come best first.
function perFile(hits) {
  const seen = new Set();
  return hits.filter((hit) => {
    const key = codeLocation(hit.url)?.file ?? hit.url;
    return !seen.has(key) && seen.add(key);
  });
}

// Searches run as the text changes. While some are running, the oldest is
// kept, since it should answer first and fill the list meanwhile, and so is
// the newest; any between them are killed (the user, 2026-10-06). A result
// older than the one shown is dropped.
function run(source) {
  const pick = vscode.window.createQuickPick();
  pick.placeholder = `Semantic search (${source}), from ${MIN_LETTERS} letters`;
  pick.matchOnDescription = false;
  pick.matchOnDetail = false;
  // VS Code otherwise moves items whose label matches the typed text to the
  // top. A proposed API in 1.137; when the host refuses it, the order can
  // still change where a label matches.
  try {
    pick.sortByLabel = false;
  } catch {}

  let timer = null;
  let seq = 0;
  let shown = 0;
  let flights = [];

  const busy = () => (pick.busy = flights.length > 0);
  const launch = (text) => {
    const flight = { seq: ++seq, ...search(source, text) };
    flights.push(flight);
    for (const between of flights.slice(1, -1)) between.child.kill();
    flights = flights.length > 2 ? [flights[0], flights[flights.length - 1]] : flights;
    busy();
    flight.done.then(
      ({ hits, warning }) => {
        if (flight.seq < shown) return;
        shown = flight.seq;
        pick.items = perFile(hits).map(item);
        pick.title = [warning, hits.length === 0 ? 'no hits' : ''].filter(Boolean).join('  ') || undefined;
      },
      (e) => {
        if (!e.killed && flight.seq > shown) pick.title = `semsearch: ${e.message}`;
      },
    ).finally(() => {
      flights = flights.filter((f) => f !== flight);
      busy();
    });
  };

  pick.onDidChangeValue((value) => {
    clearTimeout(timer);
    const text = value.trim();
    if (text.length < MIN_LETTERS) return;
    timer = setTimeout(() => launch(text), DEBOUNCE_MS);
  });
  pick.onDidAccept(() => {
    const [chosen] = pick.activeItems;
    if (chosen) {
      pick.hide();
      return open(chosen.hit);
    }
    // Enter before the pause ends searches at once.
    clearTimeout(timer);
    const text = pick.value.trim();
    if (text.length >= MIN_LETTERS) launch(text);
  });
  pick.onDidHide(() => {
    clearTimeout(timer);
    flights.forEach((f) => f.child.kill());
    pick.dispose();
  });
  pick.show();
}

// Changes under these never change what code search sees: .gitignore keeps
// build output out of the index, and this keeps it from starting a run.
const IGNORED = /[\\/](\.git|target|node_modules|dist|build|\.direnv|result)[\\/]/;
const QUIET_MS = 10_000;

// One re-index per folder at a time, QUIET_MS after its last change. Changes
// during a run start one more run when it ends.
function watchFolder(folder, output) {
  let timer = null;
  let running = false;
  let again = false;
  const index = () => {
    if (running) return void (again = true);
    running = true;
    execFile(binary(), ['index-code', folder.uri.fsPath], { maxBuffer: 16 << 20 }, (err, _stdout, stderr) => {
      running = false;
      if (stderr.trim()) output.appendLine(stderr.trim());
      if (err) output.appendLine(`index-code ${folder.uri.fsPath} failed: ${err.message}`);
      if (again) {
        again = false;
        schedule();
      }
    });
  };
  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(index, QUIET_MS);
  };
  const changed = (uri) => {
    if (!IGNORED.test(uri.fsPath)) schedule();
  };
  const watcher = vscode.workspace.createFileSystemWatcher(new vscode.RelativePattern(folder, '**/*'));
  watcher.onDidChange(changed);
  watcher.onDidCreate(changed);
  watcher.onDidDelete(changed);
  return { dispose: () => (clearTimeout(timer), watcher.dispose()) };
}

function activate(context) {
  context.subscriptions.push(
    vscode.commands.registerCommand('semanticSearch.code', () => run('code')),
    vscode.commands.registerCommand('semanticSearch.all', () => run('all')),
  );
  if (!vscode.workspace.getConfiguration('semanticSearch').get('watch')) return;
  const output = vscode.window.createOutputChannel('Semantic Search');
  const watchers = new Map();
  const add = (folder) => watchers.set(folder.uri.toString(), watchFolder(folder, output));
  (vscode.workspace.workspaceFolders || []).forEach(add);
  context.subscriptions.push(
    output,
    vscode.workspace.onDidChangeWorkspaceFolders((e) => {
      e.added.forEach(add);
      for (const folder of e.removed) {
        watchers.get(folder.uri.toString())?.dispose();
        watchers.delete(folder.uri.toString());
      }
    }),
    { dispose: () => watchers.forEach((w) => w.dispose()) },
  );
}

module.exports = { activate };
