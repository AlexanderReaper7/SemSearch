<script>
  let items = $state([]);
</script>

<!-- First. -->
{#if items.length === 0}
  <p>Nothing yet.</p>
{/if}

<!-- Second. -->
<ul>
  {#each items as item}
    <li>{item}</li>
  {/each}
</ul>
