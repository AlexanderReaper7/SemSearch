<script>
  // The aurora over the top of its parent while `working`, faded in and out.
  // Frames are drawn only while it is visible, including the fade-out.
  import { onMount } from 'svelte';
  import { aurora } from './aurora.js';

  let { working = false, height = '55%' } = $props();

  const FADE_MS = 900;
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
  let canvas;
  let error = $state('');
  let draw = null;
  let raf = 0;
  let fadingUntil = 0;
  const t0 = performance.now();

  function frame(now) {
    draw(reduced ? 0 : (now - t0) / 1000);
    raf = working || now < fadingUntil ? requestAnimationFrame(frame) : 0;
  }

  onMount(() => {
    try {
      const made = aurora(canvas);
      if (made instanceof Error) error = made.message;
      else draw = made;
    } catch (e) {
      error = e.message;
    }
    return () => cancelAnimationFrame(raf);
  });

  $effect(() => {
    if (!draw) return;
    if (!working) fadingUntil = performance.now() + FADE_MS + 100;
    if (!raf) raf = requestAnimationFrame(frame);
  });
</script>

<canvas bind:this={canvas} class:on={working} style:height aria-hidden="true"></canvas>
{#if error}<p class="error">Aurora unavailable: {error}</p>{/if}

<style>
  canvas {
    position: absolute;
    left: 0;
    top: 0;
    width: 100%;
    z-index: -1;
    pointer-events: none;
    mix-blend-mode: screen;
    opacity: 0;
    transition: opacity 0.9s ease;
  }
  canvas.on {
    opacity: 1;
  }
  .error {
    position: absolute;
    bottom: 4px;
    left: 8px;
    margin: 0;
    color: #f85149;
    font-size: 12px;
  }
</style>
