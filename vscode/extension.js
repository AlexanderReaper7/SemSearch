// Asks semsearch for hits and lists them in a quick pick. Code hits carry a
// `vscode://file/<path>:<line>:1` URL and open at that line; web and file hits
// open in the default handler.

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
      resolve(stdout.split('\n').filter(Boolean).map((line) => JSON.parse(line)));
    });
  });
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
      description: `${hit.similarity.toFixed(3)}  ${hit.source}`,
      detail: hit.chunk.replace(/\s+/g, ' ').slice(0, 200),
      hit,
    })),
    { matchOnDescription: true, matchOnDetail: true, placeHolder: query },
  );
  if (picked) await open(picked.hit);
}

function activate(context) {
  context.subscriptions.push(
    vscode.commands.registerCommand('semanticSearch.code', () => run('code')),
    vscode.commands.registerCommand('semanticSearch.all', () => run('all')),
  );
}

module.exports = { activate };
