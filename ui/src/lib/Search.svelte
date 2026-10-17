<script>
  // The search popup: the black pill, the aurora behind it while a search
  // runs, and the hits under it. The same component runs in the Tauri window
  // and in VS Code's modal; `backend` is what differs (src/backend-*.js).
  //
  // Searches run as the text changes, from the second letter, 300 ms after
  // the last key (the user, 2026-10-06). A result older than the one shown
  // is dropped.
  import { onMount } from 'svelte';
  import Aurora from './Aurora.svelte';
  import Mark from './Mark.svelte';

  let { backend, sources = ['all', 'files', 'web', 'code'], source = $bindable(sources[0]) } = $props();

  const MIN_LETTERS = 2;
  const DEBOUNCE_MS = 300;

  let text = $state('');
  let hits = $state([]);
  let note = $state('');
  let selected = $state(0);
  let inFlight = $state(0);
  let input;
  let list;
  let timer = 0;
  let seq = 0;
  let shown = 0;

  const working = $derived(inFlight > 0);

  async function run(query) {
    const mine = ++seq;
    inFlight++;
    try {
      const out = await backend.search(query, source);
      if (mine < shown) return;
      shown = mine;
      hits = out.hits;
      selected = 0;
      note = out.warning || (out.hits.length === 0 ? 'no hits' : '');
    } catch (e) {
      if (mine > shown) note = String(e.message ?? e);
    } finally {
      inFlight--;
    }
  }

  function changed() {
    clearTimeout(timer);
    const query = text.trim();
    if (query.length < MIN_LETTERS) return;
    timer = setTimeout(() => {
      timer = 0;
      run(query);
    }, DEBOUNCE_MS);
  }

  function now() {
    clearTimeout(timer);
    timer = 0;
    const query = text.trim();
    if (query.length >= MIN_LETTERS) run(query);
  }

  function open(hit) {
    if (hit) backend.open(hit);
  }

  function keydown(e) {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (hits.length === 0) return;
      selected = (selected + (e.key === 'ArrowDown' ? 1 : hits.length - 1)) % hits.length;
      list?.children[selected]?.scrollIntoView({ block: 'nearest' });
    } else if (e.key === 'Enter') {
      e.preventDefault();
      // Enter opens the selected hit once there are hits for this text, and
      // before that searches at once instead of waiting out the pause.
      if (hits.length && !timer) open(hits[selected]);
      else now();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      backend.close();
    } else if (e.key === 'Tab' && sources.length > 1) {
      e.preventDefault();
      const k = sources.indexOf(source);
      source = sources[(k + (e.shiftKey ? sources.length - 1 : 1)) % sources.length];
      now();
    }
  }

  // Unix seconds -> `2026-09-22 14:03` in local time.
  function date(unix) {
    if (!unix) return '';
    const d = new Date(unix * 1000);
    const p = (n) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
  }

  onMount(() => {
    input.focus();
    // A backend can ask for the input again, as when the window is shown.
    backend.onShow?.(() => {
      input.focus();
      input.select();
    });
  });
</script>

<div class="view" class:working>
  <Aurora {working} />
  <div class="pill">
    <input
      bind:this={input}
      bind:value={text}
      oninput={changed}
      onkeydown={keydown}
      placeholder="Search by meaning"
      spellcheck="false"
      autocomplete="off"
    />
    {#if sources.length > 1}
      <span class="source">{source}</span>
    {/if}
    <Mark {working} onclick={now} />
  </div>
  {#if note}<p class="note">{note}</p>{/if}
  <ol class="hits" bind:this={list}>
    {#each hits as hit, k (hit.url)}
      <li class:selected={k === selected}>
        <button onclick={() => open(hit)} onmousemove={() => (selected = k)}>
          <span class="title">{backend.label?.(hit) ?? hit.title ?? hit.url}</span>
          <span class="when">{date(hit.updated)}</span>
          <span class="chunk">{hit.chunk.replace(/\s+/g, ' ').slice(0, 220)}</span>
        </button>
      </li>
    {/each}
  </ol>
</div>

<style>
  .view {
    position: relative;
    isolation: isolate;
    overflow: hidden;
    display: flex;
    flex-direction: column;
    height: 100%;
    background: #000;
    color: #e6edf3;
    font: 14px/1.45 system-ui, sans-serif;
  }
  /* Opaque black, so the aurora never sits behind the text. */
  .pill {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 18px 18px 10px;
    padding: 10px 16px;
    border: 1px solid #ffffff22;
    border-radius: 24px;
    background: #000;
  }
  input {
    flex: 1;
    min-width: 0;
    padding: 0;
    border: 0;
    outline: 0;
    background: none;
    color: inherit;
    font: inherit;
    font-size: 16px;
  }
  .source {
    color: #8b949e;
    font-size: 12px;
  }
  .note {
    margin: 0 30px 6px;
    color: #8b949e;
    font-size: 12px;
  }
  .hits {
    flex: 1;
    overflow-y: auto;
    margin: 0;
    padding: 0 10px 10px;
    list-style: none;
  }
  .hits button {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: 2px 12px;
    width: 100%;
    padding: 8px 12px;
    border: 0;
    border-radius: 8px;
    background: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .selected button {
    background: #ffffff14;
  }
  .title {
    overflow: hidden;
    color: #e6edf3;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .when {
    color: #6e7681;
    font-size: 12px;
  }
  .chunk {
    grid-column: 1 / -1;
    overflow: hidden;
    color: #8b949e;
    font-size: 12px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
