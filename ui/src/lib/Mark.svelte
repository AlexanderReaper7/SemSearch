<script>
  // The search button is icon 9c, grey at rest. While working the lens roams
  // the embedding grid as if looking for something, and the lit cell takes
  // the aurora's color (icon/working.html, "Moving grid").
  import { onDestroy } from 'svelte';
  import { rainbow } from './aurora.js';

  let { working = false, size = 24, onclick } = $props();

  // The 7-pitch cells under a barrel distortion, clipped to the lens. Cell
  // (i, j) is at i * 7, j * 7 on the grid, so with the lens at (0, 0) this is
  // the icon.
  const LENS_IN = 14.6;
  const SQ = 0.2;
  const RW = LENS_IN / (1 - SQ);
  const CORE = [0.3, 0.75, 0.45, 0.55, 1, 0.25, 0.8, 0.35, 0.6];
  const cellOpacity = (i, j) => {
    if (Math.abs(i) <= 1 && Math.abs(j) <= 1) return CORE[(j + 1) * 3 + i + 1];
    const h = Math.sin(i * 12.9898 + j * 78.233) * 43758.5453;
    return 0.15 + 0.7 * (h - Math.floor(h));
  };
  const r2 = (v) => +v.toFixed(2);
  const warp = (x, y) => {
    const dx = x - 26;
    const dy = y - 26;
    const r = Math.hypot(dx, dy);
    if (r === 0) return [26, 26];
    const s = (r >= RW ? LENS_IN : r * (1 - SQ * (r / RW) ** 2)) / r;
    return [r2(26 + dx * s), r2(26 + dy * s)];
  };
  function cellsSvg(ox, oy) {
    let cells = '';
    const lo = (o) => Math.floor((o - RW - 4) / 7);
    const hi = (o) => Math.ceil((o + RW + 4) / 7);
    for (let j = lo(oy); j <= hi(oy); j++)
      for (let i = lo(ox); i <= hi(ox); i++) {
        const cx = 26 + i * 7 - ox;
        const cy = 26 + j * 7 - oy;
        if (Math.hypot(cx - 26, cy - 26) > RW + 4) continue;
        const h = 2.75;
        const rr = 1.2;
        const pts = [];
        [[1, -1, -90], [1, 1, 0], [-1, 1, 90], [-1, -1, 180]].forEach(([sx, sy, a0], k) => {
          for (let t = 0; t <= 4; t++) {
            const a = ((a0 + t * 22.5) * Math.PI) / 180;
            pts.push([cx + sx * (h - rr) + rr * Math.cos(a), cy + sy * (h - rr) + rr * Math.sin(a)]);
          }
          pts.push([[cx + h, cy], [cx, cy + h], [cx - h, cy], [cx, cy - h]][k]);
        });
        const d = 'M' + pts.map(([x, y]) => warp(x, y).join(' ')).join('L') + 'Z';
        cells +=
          i === 0 && j === 0
            ? `<path class="lit" d="${d}"/>`
            : `<path d="${d}" fill-opacity="${(cellOpacity(i, j) * 0.8).toFixed(2)}"/>`;
      }
    return cells;
  }

  // The lens hops from tile to tile: a short eased move of one or two tiles
  // and a pause, within three tiles of home, nearer tiles weighing more. On
  // stop it finishes the hop it is on, skips the pause, and goes home.
  const TILE = 7;
  const ROAM = 3;
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
  let pos = $state([0, 0]);
  let hop = null;
  let raf = 0;
  let homing = false;

  function nextHop(now) {
    const [x, y] = pos.map((v) => Math.round(v / TILE));
    let to = [0, 0];
    if (!homing) {
      const options = [];
      for (let dy = -2; dy <= 2; dy++)
        for (let dx = -2; dx <= 2; dx++)
          if ((dx || dy) && Math.abs(x + dx) <= ROAM && Math.abs(y + dy) <= ROAM)
            options.push([[x + dx, y + dy], 1 / (1 + ((x + dx) ** 2 + (y + dy) ** 2) / 3)]);
      let r = Math.random() * options.reduce((sum, [, w]) => sum + w, 0);
      to = options.find(([, w]) => (r -= w) < 0)?.[0] ?? options[0][0];
    }
    const from = [...pos];
    const dist = Math.hypot(to[0] * TILE - from[0], to[1] * TILE - from[1]) / TILE;
    hop = { from, to: to.map((v) => v * TILE), t0: now, dur: 200 + 120 * dist, dwell: 180 + Math.random() * 380 };
  }
  function frame(now) {
    if (!hop) nextHop(now);
    const k = Math.min(1, (now - hop.t0) / hop.dur);
    const e = k < 0.5 ? 4 * k ** 3 : 1 - (2 - 2 * k) ** 3 / 2;
    pos = [0, 1].map((c) => hop.from[c] + (hop.to[c] - hop.from[c]) * e);
    if (now - hop.t0 >= hop.dur + (homing ? 0 : hop.dwell)) {
      const home = homing && hop.to[0] === 0 && hop.to[1] === 0;
      hop = null;
      if (home) {
        homing = false;
        raf = 0;
        return;
      }
    }
    raf = requestAnimationFrame(frame);
  }
  $effect(() => {
    if (reduced) return;
    if (working) {
      homing = false;
      if (!raf) raf = requestAnimationFrame(frame);
    } else if (raf) homing = true;
  });
  onDestroy(() => cancelAnimationFrame(raf));

  const cells = $derived(cellsSvg(pos[0], pos[1]));
  const hot = `rgb(${rainbow(23 / 24).map(Math.round)})`;
</script>

<button class="mark" class:working style:--hot={hot} aria-label={working ? 'Searching' : 'Search'} {onclick}>
  <svg viewBox="5 5 52 52" width={size} height={size}>
    <defs><clipPath id="lens"><circle cx="26" cy="26" r={LENS_IN} /></clipPath></defs>
    <g clip-path="url(#lens)" fill="currentColor">{@html cells}</g>
    <circle cx="26" cy="26" r="17" fill="none" stroke="currentColor" stroke-width="5" />
    <path d="M38.5 38.5L52 52" stroke="currentColor" stroke-width="7" stroke-linecap="round" />
  </svg>
</button>

<style>
  .mark {
    flex: none;
    display: block;
    padding: 0;
    border: 0;
    background: none;
    color: #e6edf3;
    cursor: pointer;
  }
  svg {
    display: block;
    overflow: visible;
  }
  .mark :global(.lit) {
    fill: currentColor;
    transition: fill 0.45s;
  }
  .working :global(.lit) {
    fill: var(--hot);
  }
</style>
