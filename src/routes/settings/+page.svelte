<script lang="ts">
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import { settings, PROVIDERS } from "$lib/settings.svelte";
  import { ArrowLeft, ChevronDown, Eye, EyeOff, Check, Loader2, AlertCircle, RotateCcw } from "@lucide/svelte";

  let showKey = $state(false);
  let showEmbedKey = $state(false);

  onMount(() => {
    settings.load();
  });

  const inputClass =
    "w-full bg-[#1a1d24] border border-zinc-800/80 rounded-xl px-3 py-2 text-xs text-zinc-100 placeholder:text-zinc-600 focus:outline-none focus:border-emerald-500/50 focus:ring-1 focus:ring-emerald-500/20 transition-all";
  const labelClass = "text-[10px] font-semibold text-zinc-400 tracking-wider uppercase";
</script>

<div class="h-dvh w-screen overflow-y-auto bg-[#0e0f12] text-zinc-100 font-sans antialiased">
  <header class="sticky top-0 z-10 h-14 border-b border-zinc-800/40 px-5 flex items-center justify-between bg-[#13151a]/80 backdrop-blur-md">
    <div class="flex items-center gap-3">
      <button
        onclick={() => goto("/")}
        class="flex items-center gap-1.5 px-3 py-1.5 rounded-xl bg-zinc-800/60 hover:bg-zinc-800 text-zinc-300 border border-zinc-700/50 text-xs font-medium transition-all cursor-pointer"
      >
        <ArrowLeft class="w-3.5 h-3.5" />
        <span>Back to chat</span>
      </button>
      <h1 class="font-bold text-sm tracking-tight">Settings</h1>
    </div>

    <div class="text-[11px] font-mono flex items-center gap-1.5 min-w-24 justify-end" aria-live="polite">
      {#if settings.saveState === "pending" || settings.saveState === "saving"}
        <Loader2 class="w-3.5 h-3.5 animate-spin text-zinc-400" />
        <span class="text-zinc-400">Saving...</span>
      {:else if settings.saveState === "saved"}
        <Check class="w-3.5 h-3.5 text-emerald-400" />
        <span class="text-emerald-400">Saved</span>
      {:else if settings.saveState === "error"}
        <AlertCircle class="w-3.5 h-3.5 text-red-400" />
        <span class="text-red-400">Save failed</span>
      {/if}
    </div>
  </header>

  <main class="max-w-2xl mx-auto p-5 md:p-8 flex flex-col gap-8">
    {#if settings.saveState === "error"}
      <div class="text-xs text-red-300 bg-red-500/10 border border-red-500/30 rounded-xl px-4 py-3 break-words">
        Couldn't save settings: {settings.saveError}
      </div>
    {/if}

    <!-- Provider -->
    <section class="flex flex-col gap-4">
      <div>
        <h2 class="font-bold text-sm">Provider</h2>
        <p class="text-[11px] text-zinc-500 mt-0.5">The endpoint used for chat. Changes are saved automatically.</p>
      </div>

      <div class="flex flex-col gap-1.5">
        <label for="provider-select" class={labelClass}>Provider type</label>
        <div class="relative">
          <select
            id="provider-select"
            value={settings.provider}
            onchange={(e) => settings.setProvider(e.currentTarget.value)}
            class="{inputClass} appearance-none cursor-pointer font-medium"
          >
            {#each PROVIDERS as p}
              <option value={p.id} class="bg-[#1a1d24] text-zinc-100">{p.label}</option>
            {/each}
          </select>
          <div class="pointer-events-none absolute inset-y-0 right-0 flex items-center px-3 text-zinc-400">
            <ChevronDown class="w-3.5 h-3.5" />
          </div>
        </div>
      </div>

      <div class="flex flex-col gap-1.5">
        <div class="flex items-center justify-between">
          <label for="base-url-input" class={labelClass}>Base URL</label>
          <button
            onclick={() => settings.resetBaseUrl()}
            class="flex items-center gap-1 text-[10px] text-zinc-500 hover:text-emerald-400 cursor-pointer"
            title="Reset to the provider's default URL"
          >
            <RotateCcw class="w-3 h-3" />
            Default
          </button>
        </div>
        <input id="base-url-input" type="text" bind:value={settings.baseUrl} spellcheck="false" class="{inputClass} font-mono" />
      </div>

      {#if settings.provider !== "ollama"}
        <div class="flex flex-col gap-1.5">
          <label for="api-key-input" class={labelClass}>API Key</label>
          <div class="relative">
            <input
              id="api-key-input"
              type={showKey ? "text" : "password"}
              bind:value={settings.apiKey}
              placeholder="sk-..."
              autocomplete="off"
              spellcheck="false"
              class="{inputClass} font-mono pr-10"
            />
            <button
              type="button"
              onclick={() => (showKey = !showKey)}
              title={showKey ? "Hide key" : "Show key"}
              class="absolute inset-y-0 right-0 px-3 text-zinc-500 hover:text-zinc-200 cursor-pointer"
            >
              {#if showKey}<EyeOff class="w-3.5 h-3.5" />{:else}<Eye class="w-3.5 h-3.5" />{/if}
            </button>
          </div>
          <p class="text-[10px] text-zinc-600">The key is stored as plain text in this app's data folder on this machine.</p>
        </div>
      {/if}

      <div class="flex flex-col gap-1.5">
        <label for="model-name-input" class={labelClass}>Model</label>
        <input id="model-name-input" type="text" bind:value={settings.model} spellcheck="false" class={inputClass} />
      </div>
    </section>

    <!-- Embedding -->
    <section class="flex flex-col gap-4 border-t border-zinc-800/60 pt-6">
      <div>
        <h2 class="font-bold text-sm">Embedding (RAG)</h2>
        <p class="text-[11px] text-zinc-500 mt-0.5 leading-relaxed">
          Leave Base URL empty to use the same endpoint as chat.
          {#if settings.provider === "anthropic"}
            <span class="text-amber-400">Anthropic has no embeddings API, so you must set a separate endpoint.</span>
          {/if}
          For Ollama, enter an embedding model name (e.g. <span class="font-mono">nomic-embed-text</span>).
        </p>
      </div>

      <div class="flex flex-col gap-1.5">
        <label for="embed-url-input" class={labelClass}>Embedding Base URL</label>
        <input
          id="embed-url-input"
          type="text"
          bind:value={settings.embedBaseUrl}
          placeholder="Empty = use chat Base URL"
          spellcheck="false"
          class="{inputClass} font-mono"
        />
      </div>

      <div class="flex flex-col gap-1.5">
        <label for="embed-key-input" class={labelClass}>Embedding API Key</label>
        <div class="relative">
          <input
            id="embed-key-input"
            type={showEmbedKey ? "text" : "password"}
            bind:value={settings.embedApiKey}
            placeholder="Empty = use chat API key"
            autocomplete="off"
            spellcheck="false"
            class="{inputClass} font-mono pr-10"
          />
          <button
            type="button"
            onclick={() => (showEmbedKey = !showEmbedKey)}
            title={showEmbedKey ? "Hide key" : "Show key"}
            class="absolute inset-y-0 right-0 px-3 text-zinc-500 hover:text-zinc-200 cursor-pointer"
          >
            {#if showEmbedKey}<EyeOff class="w-3.5 h-3.5" />{:else}<Eye class="w-3.5 h-3.5" />{/if}
          </button>
        </div>
      </div>

      <div class="flex flex-col gap-1.5">
        <label for="embed-model-input" class={labelClass}>Embedding Model</label>
        <input
          id="embed-model-input"
          type="text"
          bind:value={settings.embedModel}
          placeholder="Empty = endpoint default"
          spellcheck="false"
          class={inputClass}
        />
      </div>
    </section>
  </main>
</div>
