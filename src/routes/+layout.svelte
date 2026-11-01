<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import { onNavigate, afterNavigate } from '$app/navigation';
  import { installBackButton, noteNavigation } from '$lib/back';
  import { settings } from '$lib/settings.svelte';
  import SecretsGate from '$lib/SecretsGate.svelte';

  let { children } = $props();

  // Load early so the unlock prompt can appear on whichever page the app opens on
  onMount(() => {
    settings.load();

    // The browser's context menu (Back, Reload, Inspect...) gives the web away; keep it only
    // where it is useful: on text that can be selected and in form fields.
    const onContextMenu = (e: MouseEvent) => {
      const el = e.target as HTMLElement | null;
      if (!el?.closest('.selectable, input, textarea, pre, code')) e.preventDefault();
    };
    document.addEventListener('contextmenu', onContextMenu);

    // Android system Back: close overlays first, then step back, then exit
    const backCleanup = installBackButton();

    return () => {
      document.removeEventListener('contextmenu', onContextMenu);
      backCleanup.then((stop) => stop());
    };
  });

  afterNavigate((nav) => noteNavigation(nav.from !== null));

  // Cross-fade/slide between pages instead of an instant swap
  onNavigate((navigation) => {
    if (!document.startViewTransition) return;
    return new Promise((resolve) => {
      // SvelteKit waits for this promise before it swaps the page. If the browser never runs the
      // transition callback (no frame is being drawn, the window is hidden), go on without the
      // animation rather than leave the old page on screen.
      const giveUp = setTimeout(resolve, 400);
      document.startViewTransition(async () => {
        clearTimeout(giveUp);
        resolve();
        await navigation.complete;
      });
    });
  });
</script>

{@render children()}

<SecretsGate />
