<script lang="ts">
  import { goto } from "$app/navigation";
  import { invoke } from "@tauri-apps/api/core";
  import { onMount } from "svelte";
  import { settings, PROVIDERS } from "$lib/settings.svelte";
  import {
    ArrowLeft,
    ChevronDown,
    Eye,
    EyeOff,
    Check,
    Loader2,
    AlertCircle,
    AlertTriangle,
    RotateCcw,
    Plus,
    Copy,
    Trash2,
    Plug
  } from "@lucide/svelte";

  type ConnectionInfo = { models: string[]; modelsListed: boolean; latencyMs: number };
  type TestResult = { status: "idle" | "testing" | "ok" | "warn" | "error"; message: string };

  let showKey = $state(false);
  let showEmbedKey = $state(false);
  let confirmingDelete = $state(false);

  // Test results and model suggestions are kept per profile so switching profiles doesn't mix them up
  let connectionTests = $state<Record<string, TestResult>>({});
  let modelSuggestions = $state<Record<string, string[]>>({});
  let embeddingTest = $state<TestResult>({ status: "idle", message: "" });

  let active = $derived(settings.active);
  let connectionTest = $derived(connectionTests[active.id] ?? { status: "idle", message: "" });
  let suggestions = $derived(modelSuggestions[active.id] ?? []);

  // Suggestions only, nothing is applied automatically: embedding-looking models the server
  // reported, plus well-known names for common setups.
  const KNOWN_EMBEDDING_MODELS = ["text-embedding-3-small", "text-embedding-3-large", "nomic-embed-text", "mxbai-embed-large"];
  let embedSuggestions = $derived([
    ...new Set([...suggestions.filter((m) => /embed/i.test(m)), ...KNOWN_EMBEDDING_MODELS])
  ]);

  onMount(() => {
    settings.load();
  });

  // Leaving a profile cancels any pending delete confirmation
  $effect(() => {
    active.id;
    confirmingDelete = false;
  });

  async function testConnection() {
    const profile = $state.snapshot(active);
    connectionTests[profile.id] = { status: "testing", message: "Testing..." };
    try {
      const info = await invoke<ConnectionInfo>("test_connection_command", {
        provider: profile.provider,
        baseUrl: profile.baseUrl,
        apiKey: profile.apiKey
      });
      modelSuggestions[profile.id] = info.models;

      if (!info.modelsListed) {
        connectionTests[profile.id] = {
          status: "warn",
          message: `Server reachable (${info.latencyMs} ms), but it doesn't list models, so the API key and model name couldn't be verified.`
        };
      } else if (profile.model.trim() && !info.models.includes(profile.model.trim())) {
        connectionTests[profile.id] = {
          status: "warn",
          message: `Connected (${info.latencyMs} ms, ${info.models.length} models). "${profile.model}" isn't in the server's list. You can still use it.`
        };
      } else {
        connectionTests[profile.id] = {
          status: "ok",
          message: `Connected (${info.latencyMs} ms, ${info.models.length} models available).`
        };
      }
    } catch (err) {
      connectionTests[profile.id] = { status: "error", message: String(err) };
    }
  }

  async function testEmbedding() {
    const profile = $state.snapshot(active);
    embeddingTest = { status: "testing", message: "Testing..." };
    try {
      const msg = await invoke<string>("test_embedding_command", {
        provider: profile.provider,
        baseUrl: profile.baseUrl,
        apiKey: profile.apiKey,
        embedding: settings.embedding
      });
      embeddingTest = { status: "ok", message: msg };
    } catch (err) {
      embeddingTest = { status: "error", message: String(err) };
    }
  }

  function deleteProfile() {
    if (!confirmingDelete) {
      confirmingDelete = true;
      return;
    }
    settings.deleteActive();
    confirmingDelete = false;
  }

  const inputClass =
    "w-full bg-[#1a1d24] border border-zinc-800/80 rounded-xl px-3 py-2 text-xs text-zinc-100 placeholder:text-zinc-600 focus:outline-none focus:border-emerald-500/50 focus:ring-1 focus:ring-emerald-500/20 transition-all";
  const labelClass = "text-[10px] font-semibold text-zinc-400 tracking-wider uppercase";
  const iconBtnClass =
    "flex items-center gap-1.5 px-2.5 py-2 rounded-xl bg-zinc-800/60 hover:bg-zinc-800 text-zinc-300 border border-zinc-700/50 text-xs font-medium transition-all cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed";
</script>

{#snippet testBadge(result: TestResult)}
  {#if result.status !== "idle"}
    <div
      class="flex items-start gap-2 text-xs rounded-xl px-3 py-2.5 border break-words
      {result.status === 'ok' ? 'text-emerald-300 bg-emerald-500/10 border-emerald-500/30' : ''}
      {result.status === 'warn' ? 'text-amber-300 bg-amber-500/10 border-amber-500/30' : ''}
      {result.status === 'error' ? 'text-red-300 bg-red-500/10 border-red-500/30' : ''}
      {result.status === 'testing' ? 'text-zinc-300 bg-zinc-800/50 border-zinc-700/50' : ''}"
      role="status"
    >
      {#if result.status === "testing"}
        <Loader2 class="w-3.5 h-3.5 mt-px animate-spin shrink-0" />
      {:else if result.status === "ok"}
        <Check class="w-3.5 h-3.5 mt-px shrink-0" />
      {:else if result.status === "warn"}
        <AlertTriangle class="w-3.5 h-3.5 mt-px shrink-0" />
      {:else}
        <AlertCircle class="w-3.5 h-3.5 mt-px shrink-0" />
      {/if}
      <span class="min-w-0">{result.message}</span>
    </div>
  {/if}
{/snippet}

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

    <!-- Profiles -->
    <section class="flex flex-col gap-4">
      <div>
        <h2 class="font-bold text-sm">Connection profiles</h2>
        <p class="text-[11px] text-zinc-500 mt-0.5 leading-relaxed">
          Each profile keeps its own provider, URL, key and model. The selected profile is used for chat. Changes are saved automatically.
        </p>
      </div>

      <div class="flex flex-col gap-1.5">
        <label for="profile-select" class={labelClass}>Active profile</label>
        <div class="flex items-center gap-2">
          <div class="relative flex-1 min-w-0">
            <select
              id="profile-select"
              bind:value={settings.activeId}
              class="{inputClass} appearance-none cursor-pointer font-medium"
            >
              {#each settings.profiles as p (p.id)}
                <option value={p.id} class="bg-[#1a1d24] text-zinc-100">{p.name || "(unnamed)"}</option>
              {/each}
            </select>
            <div class="pointer-events-none absolute inset-y-0 right-0 flex items-center px-3 text-zinc-400">
              <ChevronDown class="w-3.5 h-3.5" />
            </div>
          </div>
          <button onclick={() => settings.addProfile()} class={iconBtnClass} title="Add a new profile">
            <Plus class="w-3.5 h-3.5" /> Add
          </button>
          <button onclick={() => settings.duplicateActive()} class={iconBtnClass} title="Duplicate this profile">
            <Copy class="w-3.5 h-3.5" /> Duplicate
          </button>
          <button
            onclick={deleteProfile}
            onblur={() => (confirmingDelete = false)}
            disabled={settings.profiles.length <= 1}
            class="{iconBtnClass} {confirmingDelete ? '!bg-red-500/20 !border-red-500/50 !text-red-300' : ''}"
            title={settings.profiles.length <= 1 ? "You need at least one profile" : "Delete this profile"}
          >
            <Trash2 class="w-3.5 h-3.5" />
            {confirmingDelete ? "Confirm?" : "Delete"}
          </button>
        </div>
      </div>

      <div class="flex flex-col gap-1.5">
        <label for="profile-name-input" class={labelClass}>Profile name</label>
        <input id="profile-name-input" type="text" bind:value={active.name} class={inputClass} />
      </div>

      <div class="flex flex-col gap-1.5">
        <label for="provider-select" class={labelClass}>Provider type</label>
        <div class="relative">
          <select
            id="provider-select"
            value={active.provider}
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
            title="Reset to the provider type's default URL"
          >
            <RotateCcw class="w-3 h-3" />
            Default
          </button>
        </div>
        <input id="base-url-input" type="text" bind:value={active.baseUrl} spellcheck="false" class="{inputClass} font-mono" />
      </div>

      {#if active.provider !== "ollama"}
        <div class="flex flex-col gap-1.5">
          <label for="api-key-input" class={labelClass}>API Key</label>
          <div class="relative">
            <input
              id="api-key-input"
              type={showKey ? "text" : "password"}
              bind:value={active.apiKey}
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
        <input
          id="model-name-input"
          type="text"
          bind:value={active.model}
          list="model-suggestions"
          placeholder="Type any model name"
          spellcheck="false"
          autocomplete="off"
          class={inputClass}
        />
        <datalist id="model-suggestions">
          {#each suggestions as m}
            <option value={m}></option>
          {/each}
        </datalist>
        <p class="text-[10px] text-zinc-600">
          Type any name. {suggestions.length
            ? `${suggestions.length} suggestions from the server are available as you type.`
            : "Run “Test connection” to get suggestions from the server."}
        </p>
      </div>

      <div class="flex flex-col gap-2">
        <div>
          <button onclick={testConnection} disabled={connectionTest.status === "testing"} class={iconBtnClass}>
            <Plug class="w-3.5 h-3.5 text-emerald-400" />
            Test connection
          </button>
        </div>
        {@render testBadge(connectionTest)}
      </div>
    </section>

    <!-- Embedding -->
    <section class="flex flex-col gap-4 border-t border-zinc-800/60 pt-6">
      <div>
        <h2 class="font-bold text-sm">Embedding (RAG)</h2>
        <p class="text-[11px] text-zinc-500 mt-0.5 leading-relaxed">
          Leave Base URL empty to use the same endpoint as the active profile.
          {#if active.provider === "anthropic"}
            <span class="text-amber-400">Anthropic has no embeddings API, so you must set a separate endpoint.</span>
          {/if}
          Use the model name your endpoint expects, e.g. <span class="font-mono">text-embedding-3-small</span> for OpenAI or
          <span class="font-mono">nomic-embed-text</span> for Ollama.
        </p>
      </div>

      <div class="flex flex-col gap-1.5">
        <label for="embed-url-input" class={labelClass}>Embedding Base URL</label>
        <input
          id="embed-url-input"
          type="text"
          bind:value={settings.embedBaseUrl}
          placeholder="Empty = use the profile's Base URL"
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
            placeholder="Empty = use the profile's API key"
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
        <label for="embed-model-input" class={labelClass}>Embedding Model (required for RAG)</label>
        <input
          id="embed-model-input"
          type="text"
          bind:value={settings.embedModel}
          list="embed-model-suggestions"
          placeholder="e.g. text-embedding-3-small"
          spellcheck="false"
          autocomplete="off"
          class={inputClass}
        />
        <datalist id="embed-model-suggestions">
          {#each embedSuggestions as m}
            <option value={m}></option>
          {/each}
        </datalist>
        {#if !settings.embedModel.trim()}
          <p class="text-[11px] text-amber-400 flex items-center gap-1.5">
            <AlertTriangle class="w-3.5 h-3.5 shrink-0" />
            RAG is disabled until an embedding model is set. No default is assumed, because the right model depends on your endpoint.
          </p>
        {:else}
          <p class="text-[10px] text-zinc-600">
            Documents indexed with a different model are skipped until you re-index them (the chat screen flags them).
          </p>
        {/if}
      </div>

      <div class="flex flex-col gap-2">
        <div>
          <button onclick={testEmbedding} disabled={embeddingTest.status === "testing"} class={iconBtnClass}>
            <Plug class="w-3.5 h-3.5 text-emerald-400" />
            Test embedding
          </button>
        </div>
        {@render testBadge(embeddingTest)}
      </div>
    </section>
  </main>
</div>
