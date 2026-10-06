// Asks semsearch for hits and lists them in a quick pick. Code hits carry a
// `vscode://file/<path>:<line>:1` URL and open at that line; web and file hits
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

function search(source, query) {
  return new Promise((resolve, reject) => {
    execFile(binary(), ['search', '--json', '-s', source, '-n', '30', query], { maxBuffer: 16 << 20 }, (err, stdout, stderr) => {
      if (err) return reject(new Error(stderr || err.message));
      // A slow search or a fallback to keyword hits is reported on stderr.
      if (stderr.trim()) vscode.window.showWarningMessage(stderr.trim());
      resolve(stdout.split('\n').filter(Boolean).map((line) => JSON.parse(line)));
    });
  });
}

// Unix seconds -> `2026-09-22 14:03` in local time.
function date(unix) {
  if (!unix) return '';
  const d = new Date(unix * 1000);
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

// vscode://file/home/x/a b.rs:12:1 -> { file: '/home/x/a b.rs', line: 12 }
function codeLocation(url) {
  const m = /^vscode:\/\/file(\/.*):(\d+):\d+$/.exec(url);
  return m && { file: decodeURIComponent(m[1]), line: Number(m[2]) };
}

async function open(hit) {
  const loc = codeLocation(hit.url);
  if (!loc) return vscode.env.openExternal(vscode.Uri.parse(hit.url));
  const doc = await vscode.workspace.openTextDocument(loc.file);
  const pos = new vscode.Position(loc.line - 1, 0);
  await vscode.window.showTextDocument(doc, { selection: new vscode.Range(pos, pos) });
}

async function run(source) {
  const query = await vscode.window.showInputBox({ prompt: `Semantic search (${source})`, ignoreFocusOut: true });
  if (!query) return;
  let hits;
  try {
    hits = await vscode.window.withProgress(
      { location: vscode.ProgressLocation.Window, title: 'Semantic search' },
      () => search(source, query),
    );
  } catch (e) {
    return vscode.window.showErrorMessage(`semsearch: ${e.message}`);
  }
  if (hits.length === 0) return vscode.window.showInformationMessage('No semantic hits.');
  const picked = await vscode.window.showQuickPick(
    hits.map((hit) => ({
      label: hit.title || hit.url,
      // Rerank score, then similarity, as on the command line.
      description: `${hit.rerank_score == null ? '-' : hit.rerank_score.toFixed(3)} ${hit.similarity == null ? 'kw' : hit.similarity.toFixed(3)}  ${hit.source}  ${date(hit.updated)}`,
      detail: hit.chunk.replace(/\s+/g, ' ').slice(0, 200),
      hit,
    })),
    { matchOnDescription: true, matchOnDetail: true, placeHolder: query },
  );
  if (picked) await open(picked.hit);
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
