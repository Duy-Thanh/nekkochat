<script lang="ts">
  import { settings } from "$lib/settings.svelte";
  import { Lock, Loader2, AlertCircle } from "@lucide/svelte";

  let passphrase = $state("");
  let busy = $state(false);
  let error = $state("");

  let visible = $derived(settings.secretsStatus === "vault-locked" && !settings.unlockDismissed);

  function focusOnMount(node: HTMLInputElement) {
    node.focus();
  }

  async function unlock() {
    if (!passphrase || busy) return;
    busy = true;
    error = "";
    try {
      await settings.unlockVault(passphrase);
      passphrase = "";
    } catch (err) {
      error = String(err);
    } finally {
      busy = false;
    }
  }

  function onKeydown(e: KeyboardEvent) {
    // isComposing: don't submit while an IME composition is in progress
    if (e.key === "Enter" && !e.isComposing) {
      e.preventDefault();
      unlock();
    }
  }
</script>

{#if visible}
  <div class="fixed inset-0 z-[60] bg-black/80 backdrop-blur-sm flex items-center justify-center p-4" role="dialog" aria-modal="true" aria-labelledby="unlock-title">
    <div class="bg-[#13151a] border border-zinc-800 rounded-2xl w-full max-w-sm p-5 flex flex-col gap-4 shadow-2xl">
      <div class="flex items-center gap-2">
        <Lock class="w-4 h-4 text-emerald-400" />
        <h3 id="unlock-title" class="font-bold text-sm text-zinc-100">Unlock your API keys</h3>
      </div>

      <p class="text-[11px] text-zinc-400 leading-relaxed">
        Your API keys are stored in an encrypted vault. Enter the master password to use them.
      </p>

      <input
        type="password"
        use:focusOnMount
        bind:value={passphrase}
        onkeydown={onKeydown}
        placeholder="Master password"
        aria-label="Master password"
        autocomplete="current-password"
        class="w-full bg-[#1a1d24] border border-zinc-800/80 rounded-xl px-3 py-2 text-xs text-zinc-100 placeholder:text-zinc-600 focus:outline-none focus:border-emerald-500/50 focus:ring-1 focus:ring-emerald-500/20"
      />

      {#if error}
        <p class="text-[11px] text-red-300 flex items-start gap-1.5" role="alert">
          <AlertCircle class="w-3.5 h-3.5 mt-px shrink-0" />
          <span>{error}</span>
        </p>
      {/if}

      <div class="flex items-center justify-between gap-3">
        <button
          onclick={() => (settings.unlockDismissed = true)}
          class="text-[11px] text-zinc-500 hover:text-zinc-300 underline underline-offset-2 cursor-pointer"
        >
          Continue without keys
        </button>
        <button
          onclick={unlock}
          disabled={!passphrase || busy}
          class="flex items-center gap-1.5 bg-emerald-500 hover:bg-emerald-400 disabled:bg-zinc-800/50 disabled:text-zinc-600 text-zinc-950 font-semibold px-4 py-2 rounded-xl text-xs transition-all cursor-pointer disabled:cursor-not-allowed"
        >
          {#if busy}<Loader2 class="w-3.5 h-3.5 animate-spin" />{/if}
          {busy ? "Unlocking..." : "Unlock"}
        </button>
      </div>
    </div>
  </div>
{/if}
