// The quick pick's search-as-you-type rules, against a fake vscode module and
// a fake semsearch (fake-semsearch.js). `node --test vscode/test/*.test.js`
const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const Module = require('module');

function emitter() {
  const listeners = [];
  const event = (fn) => listeners.push(fn);
  event.fire = (v) => listeners.forEach((fn) => fn(v));
  return event;
}

let pick;
const fakeVscode = {
  window: {
    createQuickPick() {
      pick = {
        items: [], value: '', busy: false, title: undefined, activeItems: [],
        onDidChangeValue: emitter(), onDidAccept: emitter(), onDidHide: emitter(),
        show() {}, hide() { this.onDidHide.fire(); }, dispose() {},
      };
      return pick;
    },
    showWarningMessage() {}, showErrorMessage() {},
  },
  workspace: {
    getConfiguration: () => ({ get: (k) => (k === 'binary' ? path.join(__dirname, 'fake-semsearch.js') : false) }),
    workspaceFolders: [{ uri: { fsPath: '/x' } }, { uri: { fsPath: '/y' } }],
    asRelativePath: (file) => file.replace(/^\/x\//, ''),
  },
  commands: { registerCommand: (id, fn) => (commands[id] = fn) },
};
const commands = {};
const load = Module._load;
Module._load = (request, ...rest) => (request === 'vscode' ? fakeVscode : load(request, ...rest));
require('../extension.js').activate({ subscriptions: [] });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = path.join(os.tmpdir(), `fake-semsearch-${process.pid}.log`);
process.env.FAKE_LOG = log;
const events = () => fs.readFileSync(log, 'utf8').trim().split('\n');
const type = (value) => {
  pick.value = value;
  pick.onDidChangeValue.fire(value);
};

test.beforeEach(() => {
  fs.writeFileSync(log, '');
  commands['semanticSearch.code']();
});
test.afterEach(() => pick.hide());

test('one letter does not search, two do after the pause', async () => {
  type('a');
  await sleep(500);
  assert.deepStrictEqual(fs.readFileSync(log, 'utf8'), '');
  type('ab');
  await sleep(200);
  assert.deepStrictEqual(fs.readFileSync(log, 'utf8'), '', 'nothing before 300 ms');
  await sleep(300);
  assert.deepStrictEqual(events(), ['start ab', 'end ab']);
  assert.strictEqual(pick.items[0].label, 'ab:1');
  assert.strictEqual(pick.title, undefined);
});

test('typing within the pause searches once, for the last text', async () => {
  for (const v of ['ab', 'abc', 'abcd']) {
    type(v);
    await sleep(100);
  }
  await sleep(500);
  assert.deepStrictEqual(events(), ['start abcd', 'end abcd']);
});

test('the oldest and the newest search keep running, the ones between are killed', async () => {
  for (const v of ['q1@1500', 'q2@1500', 'q3@1500', 'q4@200']) {
    type(v);
    await sleep(400);
  }
  await sleep(2500);
  const ev = events();
  assert.ok(ev.includes('end q1@1500'), ev.join(', '));
  assert.ok(!ev.includes('end q2@1500') && !ev.includes('end q3@1500'), ev.join(', '));
  assert.ok(ev.includes('end q4@200'), ev.join(', '));
  assert.strictEqual(pick.items[0].label, 'q4@200:1');
});

test('a late answer for older text does not replace a newer one', async () => {
  type('old@1500');
  await sleep(400);
  type('new@100');
  await sleep(600);
  assert.strictEqual(pick.items[0].label, 'new@100:1');
  await sleep(1200);
  assert.ok(events().includes('end old@1500'));
  assert.strictEqual(pick.items[0].label, 'new@100:1');
});

test('the oldest answer shows while the newest runs', async () => {
  type('first@600');
  await sleep(400);
  type('second@1500');
  await sleep(700);
  assert.strictEqual(pick.items[0].label, 'first@600:1');
  assert.strictEqual(pick.busy, true);
  await sleep(1500);
  assert.strictEqual(pick.items[0].label, 'second@1500:1');
  assert.strictEqual(pick.busy, false);
});

test('code search covers the open folders', async () => {
  type('ab');
  await sleep(600);
  const args = JSON.parse(fs.readFileSync(`${log}.args`, 'utf8'));
  assert.deepStrictEqual(args.slice(-6), ['--in', '/x', '--in', '/y', '--', 'ab']);
});

test('one item per file, at its best piece, labelled by its path and line', async () => {
  type('ab#files');
  await sleep(600);
  assert.deepStrictEqual(pick.items.map((i) => i.label), ['a.rs:10', 'b.rs:2']);
  assert.deepStrictEqual(pick.items.map((i) => i.description), ['', '']);
});
