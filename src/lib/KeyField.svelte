<script lang="ts">
  import { originOf } from "$lib/settings.svelte";
  import { Eye, EyeOff, ShieldCheck, AlertTriangle, AlertCircle, Loader2, Trash2 } from "@lucide/svelte";

  /**
   * A write-only API key field. The saved key is never shown or read back: the app only knows that
   * one exists and which origin it is tied to. A key is only ever sent to that origin.
   */
  let {
    id,
    label,
    hasKey,
    keyOrigin,
    currentUrl,
    placeholder = "sk-...",
    blockedReason = "",
    draft = $bindable(""),
    onsave,
    onclear
  }: {
    id: string;
    label: string;
    hasKey: boolean;
    keyOrigin: string;
    /** The Base URL this key will be used with */
    currentUrl: string;
    placeholder?: string;
    /** When set, saving is disabled and this explains why */
    blockedReason?: string;
    /** What the user has typed but not saved yet. Bindable so a Test button can use it. */
    draft?: string;
    onsave: (key: string) => Promise<void>;
    onclear: () => Promise<void>;
  } = $props();

  let show = $state(false);
  let busy = $state(false);
  let error = $state("");
  let replacing = $state(false);
  let confirmingRemove = $state(false);

  let currentOrigin = $derived(originOf(currentUrl));
  let mismatch = $derived(hasKey && !!currentOrigin && keyOrigin !== currentOrigin);

  async function run(action: () => Promise<void>) {
    busy = true;
    error = "";
    try {
      await action();
    } catch (err) {
      error = String(err);
    } finally {
      busy = false;
    }
  }

  function save() {
    const key = draft.trim();
    if (!key || busy || blockedReason) return;
    return run(async () => {
      await onsave(key);
      draft = "";
      replacing = false;
      show = false;
    });
  }

  function remove() {
    if (!confirmingRemove) {
      confirmingRemove = true;
      return;
    }
    confirmingRemove = false;
    return run(onclear);
  }

  const labelClass = "text-[10px] font-semibold text-zinc-400 tracking-wider uppercase";
  const inputClass =
    "w-full bg-[#1a1d24] border border-zinc-800/80 rounded-xl px-3 py-2 text-xs text-zinc-100 placeholder:text-zinc-600 focus:outline-none focus:border-emerald-500/50 focus:ring-1 focus:ring-emerald-500/20 transition-all";
  const smallBtn =
    "flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg bg-zinc-800/60 hover:bg-zinc-800 text-zinc-300 border border-zinc-700/50 text-[11px] font-medium cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed";
</script>

<div class="flex flex-col gap-1.5">
  <label for={id} class={labelClass}>{label}</label>

  {#if hasKey && !replacing}
    <div class="flex items-center gap-2 bg-[#1a1d24] border border-zinc-800/80 rounded-xl px-3 py-2">
      <ShieldCheck class="w-3.5 h-3.5 text-emerald-400 shrink-0" />
      <span class="text-xs text-zinc-300 min-w-0 truncate">
        Key saved for <span class="font-mono text-zinc-200">{keyOrigin}</span>
      </span>
      <span class="flex-1"></span>
      <button type="button" class={smallBtn} onclick={() => (replacing = true)}>Replace</button>
      <button
        type="button"
        class="{smallBtn} {confirmingRemove ? '!bg-red-500/20 !border-red-500/50 !text-red-300' : ''}"
        onclick={remove}
        onblur={() => (confirmingRemove = false)}
        disabled={busy}
      >
        <Trash2 class="w-3 h-3" />
        {confirmingRemove ? "Confirm?" : "Remove"}
      </button>
    </div>
  {:else}
    <div class="flex items-center gap-2">
      <div class="relative flex-1 min-w-0">
        <input
          {id}
          type={show ? "text" : "password"}
          bind:value={draft}
          onkeydown={(e) => {
            // isComposing: don't submit while an IME composition is in progress
            if (e.key === "Enter" && !e.isComposing) {
              e.preventDefault();
              save();
            }
          }}
          {placeholder}
          autocomplete="off"
          spellcheck="false"
          class="{inputClass} font-mono pr-10"
        />
        <button
          type="button"
          onclick={() => (show = !show)}
          title={show ? "Hide key" : "Show key"}
          class="absolute inset-y-0 right-0 px-3 text-zinc-500 hover:text-zinc-200 cursor-pointer"
        >
          {#if show}<EyeOff class="w-3.5 h-3.5" />{:else}<Eye class="w-3.5 h-3.5" />{/if}
        </button>
      </div>
      <button type="button" class={smallBtn} onclick={save} disabled={!draft.trim() || busy || !!blockedReason}>
        {#if busy}<Loader2 class="w-3 h-3 animate-spin" />{/if}
        Save key
      </button>
      {#if replacing}
        <button
          type="button"
          class={smallBtn}
          onclick={() => {
            replacing = false;
            draft = "";
          }}
        >
          Cancel
        </button>
      {/if}
    </div>
    {#if blockedReason}
      <p class="text-[11px] text-amber-400 flex items-start gap-1.5">
        <AlertTriangle class="w-3.5 h-3.5 mt-px shrink-0" />
        <span>{blockedReason}</span>
      </p>
    {/if}
  {/if}

  {#if mismatch}
    <p class="text-[11px] text-amber-400 flex items-start gap-1.5">
      <AlertTriangle class="w-3.5 h-3.5 mt-px shrink-0" />
      <span>
        This key is tied to <span class="font-mono">{keyOrigin}</span> and will not be sent to
        <span class="font-mono">{currentOrigin}</span>. Replace it to use it with this URL.
      </span>
    </p>
  {/if}
  {#if error}
    <p class="text-[11px] text-red-300 flex items-start gap-1.5" role="alert">
      <AlertCircle class="w-3.5 h-3.5 mt-px shrink-0" />
      <span class="min-w-0 break-words">{error}</span>
    </p>
  {/if}
</div>
