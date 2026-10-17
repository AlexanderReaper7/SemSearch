// The Tauri window. Searches go to app/src/main.rs, which calls the semsearch
// library; a chosen hit opens in its default handler (a code hit's
// vscode://file URL opens in VS Code) and the window hides.
import { mount } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import Search from './lib/Search.svelte';

const win = getCurrentWindow();

const backend = {
  search: async (query, source) => {
    const { hits, notes } = await invoke('search', { query, source });
    return { hits, warning: notes.join('  ') };
  },
  open: async (hit) => {
    await invoke('open', { url: hit.url });
    await win.hide();
  },
  close: () => win.hide(),
  onShow: (focus) => win.onFocusChanged(({ payload: focused }) => focused && focus()),
};

mount(Search, { target: document.getElementById('root'), props: { backend } });
