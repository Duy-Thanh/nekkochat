<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import { settings } from '$lib/settings.svelte';
  import SecretsGate from '$lib/SecretsGate.svelte';

  let { children } = $props();

  // Load early so the unlock prompt can appear on whichever page the app opens on
  onMount(() => {
    settings.load();
  });
</script>

{@render children()}

<SecretsGate />
