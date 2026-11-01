<script lang="ts">
  import 'highlight.js/styles/github-dark.css';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { renderMarkdown } from '$lib/markdown';

  let { content = '' } = $props();

  // Open chat links in the system browser instead of navigating the app's webview.
  // Handles middle-click too, which would otherwise try to open a new window.
  function handleClick(e: MouseEvent) {
    const anchor = (e.target as HTMLElement).closest('a');
    if (!anchor) return;
    e.preventDefault();
    const href = anchor.getAttribute('href');
    if (e.button <= 1 && href && /^(https?:|mailto:)/i.test(href)) openUrl(href).catch(console.error);
  }

  // Model output is untrusted content; renderMarkdown sanitizes it before {@html}
  let renderedHtml = $derived(renderMarkdown(content));
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div onclick={handleClick} onauxclick={handleClick} class="markdown-body max-w-none text-xs md:text-sm leading-relaxed whitespace-normal">
  {@html renderedHtml}
</div>

<style>
  :global(.markdown-body table) {
    width: 100%;
    border-collapse: collapse;
    margin-top: 1rem;
    margin-bottom: 1rem;
    font-size: 0.8125rem;
    background-color: #13151a;
    border-radius: 0.75rem;
    overflow: hidden;
    border: 1px solid rgba(63, 63, 70, 0.5);
  }

  :global(.markdown-body th) {
    background-color: #1a1d24;
    color: #e4e4e7;
    font-weight: 600;
    text-align: left;
    padding: 0.625rem 0.875rem;
    border-bottom: 1px solid rgba(63, 63, 70, 0.6);
  }

  :global(.markdown-body td) {
    padding: 0.625rem 0.875rem;
    color: #d4d4d8;
    border-bottom: 1px solid rgba(63, 63, 70, 0.3);
  }

  :global(.markdown-body tr:last-child td) {
    border-bottom: none;
  }

  :global(.markdown-body tr:nth-child(even)) {
    background-color: rgba(24, 27, 34, 0.5);
  }

  :global(.markdown-body p) {
    margin-bottom: 0.75rem;
  }

  :global(.markdown-body p:last-child) {
    margin-bottom: 0;
  }

  :global(.markdown-body pre) {
    margin: 0.75rem 0;
    padding: 0.875rem 1rem;
    overflow-x: auto;
    background-color: #0d1117;
    border: 1px solid rgba(63, 63, 70, 0.5);
    border-radius: 0.75rem;
    font-size: 0.75rem;
    line-height: 1.6;
  }

  :global(.markdown-body pre code.hljs) {
    padding: 0;
    background: transparent;
  }

  :global(.markdown-body :not(pre) > code) {
    padding: 0.125rem 0.375rem;
    background-color: rgba(63, 63, 70, 0.5);
    border-radius: 0.375rem;
    font-size: 0.8em;
  }

  :global(.markdown-body ul) {
    list-style: disc;
    padding-left: 1.5rem;
    margin-bottom: 0.75rem;
  }

  :global(.markdown-body ol) {
    list-style: decimal;
    padding-left: 1.5rem;
    margin-bottom: 0.75rem;
  }

  :global(.markdown-body h1),
  :global(.markdown-body h2),
  :global(.markdown-body h3) {
    font-weight: 700;
    color: #f4f4f5;
    margin: 1rem 0 0.5rem;
  }

  :global(.markdown-body h1) { font-size: 1.25rem; }
  :global(.markdown-body h2) { font-size: 1.1rem; }
  :global(.markdown-body h3) { font-size: 1rem; }

  :global(.markdown-body a) {
    color: #34d399;
    text-decoration: underline;
  }

  :global(.markdown-body blockquote) {
    border-left: 3px solid rgba(16, 185, 129, 0.5);
    padding-left: 0.875rem;
    color: #a1a1aa;
    margin: 0.75rem 0;
  }
</style>
