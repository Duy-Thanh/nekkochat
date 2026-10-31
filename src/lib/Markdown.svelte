<script lang="ts">
  import { marked } from 'marked';

  let { content = '' } = $props();

  // Cau hinh marked de render table mượt hơn
  marked.setOptions({
    gfm: true,
    breaks: true
  });

  let renderedHtml = $derived(marked.parse(content));
</script>

<div class="markdown-body prose prose-invert max-w-none text-xs md:text-sm leading-relaxed">
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
</style>