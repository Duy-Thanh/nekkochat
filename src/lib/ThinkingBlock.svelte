<script lang="ts">
  import { Brain, ChevronRight } from '@lucide/svelte';

  let { thinkingText = '', isGenerating = false } = $props();
  let isOpen = $state(true);
</script>

{#if thinkingText.trim()}
  <div class="mb-3 rounded-xl border border-zinc-800/80 bg-[#13151a]/60 overflow-hidden text-xs transition-all">
    <button 
      onclick={() => isOpen = !isOpen}
      class="w-full px-3.5 py-2 flex items-center justify-between bg-[#181b22]/80 hover:bg-[#1a1d24] text-zinc-400 hover:text-zinc-200 transition-colors"
    >
      <div class="flex items-center gap-2 font-medium">
        <Brain class="w-3.5 h-3.5 text-emerald-400 {isGenerating ? 'animate-pulse' : ''}" />
        <span>{isGenerating ? 'AI đang suy nghĩ...' : 'Quá trình suy luận'}</span>
      </div>
      <ChevronRight class="w-3.5 h-3.5 transform transition-transform duration-200 {isOpen ? 'rotate-90' : ''}" />
    </button>

    {#if isOpen}
      <div class="p-3.5 text-zinc-400 font-mono text-[11px] leading-relaxed whitespace-pre-wrap border-t border-zinc-800/50 bg-[#0e0f12]/40 max-h-60 overflow-y-auto">
        {thinkingText}
      </div>
    {/if}
  </div>
{/if}