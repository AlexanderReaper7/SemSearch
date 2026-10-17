// The webview in VS Code's modal. The extension runs the searches and opens
// hits; this page talks to it through postMessage (vscode/extension.js).
import { mount } from 'svelte';
import Search from './lib/Search.svelte';

const vscode = acquireVsCodeApi();
const pending = new Map();
let next = 0;
let showListener = null;

window.addEventListener('message', ({ data }) => {
  if (data.type === 'result') {
    const p = pending.get(data.id);
    pending.delete(data.id);
    if (data.error) p?.reject(new Error(data.error));
    else p?.resolve({ hits: data.hits, warning: data.warning });
  } else if (data.type === 'show') {
    showListener?.();
  }
});

const backend = {
  search: (query, source) =>
    new Promise((resolve, reject) => {
      const id = ++next;
      pending.set(id, { resolve, reject });
      vscode.postMessage({ type: 'search', id, query, source });
    }),
  open: (hit) => vscode.postMessage({ type: 'open', url: hit.url }),
  close: () => vscode.postMessage({ type: 'close' }),
  onShow: (focus) => (showListener = focus),
  // Code hits by their path in the open folder and line, as the quick pick
  // showed them; the extension sends that label along with each hit.
  label: (hit) => hit.label,
};

mount(Search, { target: document.getElementById('root'), props: { backend, sources: ['code'] } });
