<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { settings } from "$lib/settings.svelte";
  import Markdown from '$lib/Markdown.svelte';
  import ThinkingBlock from '$lib/ThinkingBlock.svelte';
  import { SYSTEM_PRESETS, type SystemPreset } from '$lib/presets';

  import {
    X,
    Menu,
    Settings as SettingsIcon,
    Send,
    Plus,
    MessageSquare,
    Trash2,
    Paperclip,
    FileText,
    Loader2,
    SlidersHorizontal,
    Sparkles,
    Square,
    RotateCcw,
    Copy,
    Check,
    AlertTriangle,
    Pencil,
    Search
  } from '@lucide/svelte';

  type Message = {
    role: "user" | "assistant" | "system";
    content: string;
  };

  type ChatSession = {
    id: string;
    title: string;
    updatedAt: number;
    systemPrompt: string;
    messages: Message[];
  };

  type DocumentInfo = { fileName: string; chunks: number; model: string; sourcePath: string };

  let inputPrompt = $state("");

  // Sessions State
  let sessions = $state<ChatSession[]>([]);
  let currentSessionId = $state<string>("");
  // Mỗi session stream độc lập nên có thể chuyển session trong lúc đang trả lời
  let generatingIds = $state<string[]>([]);
  let loadError = $state("");

  // Presets UI State
  let isPresetModalOpen = $state(false);

  // RAG State
  let isIndexing = $state(false);
  let documents = $state<DocumentInfo[]>([]);
  let ragStatusMsg = $state("");
  let ragStatusIsWarning = $state(false);
  let ragStatusTimer: ReturnType<typeof setTimeout> | undefined;

  let isSidebarOpen = $state(false);
  let copiedIndex = $state<number | null>(null);

  let chatContainer: HTMLDivElement;
  let inputEl: HTMLTextAreaElement;

  let currentSession = $derived.by(() => {
    return sessions.find(s => s.id === currentSessionId);
  });

  let currentMessages = $derived.by(() => {
    return currentSession ? currentSession.messages : [];
  });

  let isGenerating = $derived(generatingIds.includes(currentSessionId));

  let lastMessage = $derived(currentMessages[currentMessages.length - 1]);

  function scrollToBottom() {
    setTimeout(() => {
      if (chatContainer) chatContainer.scrollTop = chatContainer.scrollHeight;
    }, 50);
  }

  function showRagStatus(msg: string, autoClear = true, isWarning = false) {
    clearTimeout(ragStatusTimer);
    ragStatusMsg = msg;
    ragStatusIsWarning = isWarning;
    // Warnings stay longer: they explain why documents are not being used
    if (autoClear) ragStatusTimer = setTimeout(() => ragStatusMsg = "", isWarning ? 10000 : 4000);
  }

  // A document is stale when it was embedded by a model other than the one currently configured
  function isStale(doc: DocumentInfo) {
    return !doc.model || doc.model !== settings.embedding.model;
  }

  function staleReason(doc: DocumentInfo) {
    const current = settings.embedding.model || "(not set)";
    return doc.model
      ? `Indexed with "${doc.model}", but the current embedding model is ${current}. It is skipped until re-indexed.`
      : `Indexed by an older version (embedding model unknown). It is skipped until re-indexed.`;
  }

  // ---- Persistence ----

  async function loadSessions() {
    try {
      const savedSessions = await invoke<ChatSession[]>("load_sessions");
      if (savedSessions && savedSessions.length > 0) {
        sessions = savedSessions.map(s => ({
          ...s,
          systemPrompt: s.systemPrompt || SYSTEM_PRESETS[0].prompt
        }));
        currentSessionId = sessions[0].id;
        return;
      }
    } catch (err) {
      console.error("Failed to load sessions:", err);
      loadError = String(err);
    }
    createNewSession();
  }

  async function saveSessions() {
    try {
      await invoke("save_sessions", { sessions: $state.snapshot(sessions) });
    } catch (err) {
      console.error("Failed to save sessions:", err);
    }
  }

  // Ghi đĩa theo nhịp thay vì mỗi token
  let saveTimer: ReturnType<typeof setTimeout> | undefined;
  function queueSaveSessions() {
    if (saveTimer) return;
    saveTimer = setTimeout(() => {
      saveTimer = undefined;
      saveSessions();
    }, 1000);
  }

  function createNewSession(presetPrompt?: string) {
    const newSession: ChatSession = {
      id: crypto.randomUUID(),
      title: "New Chat",
      updatedAt: Date.now(),
      systemPrompt: presetPrompt || SYSTEM_PRESETS[0].prompt,
      messages: []
    };
    sessions = [newSession, ...sessions];
    currentSessionId = newSession.id;
    isSidebarOpen = false;
    saveSessions();
  }

  function applyPreset(preset: SystemPreset) {
    if (currentSession) {
      currentSession.systemPrompt = preset.prompt;
      saveSessions();
    }
    isPresetModalOpen = false;
  }

  // ---- Chat history sidebar ----

  let historyQuery = $state("");
  let renamingId = $state<string | null>(null);
  let renameDraft = $state("");
  let confirmDeleteId = $state<string | null>(null);
  let confirmDeleteTimer: ReturnType<typeof setTimeout> | undefined;

  const DAY_MS = 86_400_000;

  // Sorted by last activity and grouped by calendar day. Message content is only read while
  // searching, so streaming tokens don't recompute this list the rest of the time.
  let historyGroups = $derived.by(() => {
    const q = historyQuery.trim().toLowerCase();
    const matches = sessions.filter(s =>
      !q || s.title.toLowerCase().includes(q) || s.messages.some(m => m.content.toLowerCase().includes(q))
    );
    matches.sort((a, b) => b.updatedAt - a.updatedAt);

    const startOfToday = new Date().setHours(0, 0, 0, 0);
    const bucket = (ts: number) =>
      ts >= startOfToday ? "Today"
      : ts >= startOfToday - DAY_MS ? "Yesterday"
      : ts >= startOfToday - 7 * DAY_MS ? "Previous 7 days"
      : "Older";

    const groups: { label: string; items: ChatSession[] }[] = [];
    for (const sess of matches) {
      const label = bucket(sess.updatedAt);
      const last = groups[groups.length - 1];
      if (last?.label === label) last.items.push(sess);
      else groups.push({ label, items: [sess] });
    }
    return groups;
  });

  function selectSession(id: string) {
    currentSessionId = id;
    isSidebarOpen = false;
  }

  function newChat() {
    // Reuse the current chat if it is still empty instead of piling up blank ones
    if (currentSession && currentSession.messages.length === 0) {
      isSidebarOpen = false;
      inputEl?.focus();
      return;
    }
    createNewSession();
    historyQuery = "";
  }

  function focusSelect(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  function startRename(sess: ChatSession, e: Event) {
    e.stopPropagation();
    renamingId = sess.id;
    renameDraft = sess.title;
  }

  function commitRename() {
    if (!renamingId) return;
    const sess = sessions.find(x => x.id === renamingId);
    const title = renameDraft.trim();
    if (sess && title && title !== sess.title) {
      sess.title = title;
      saveSessions();
    }
    renamingId = null;
  }

  function onRenameKeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === "Enter") commitRename();
    else if (e.key === "Escape") renamingId = null;
  }

  // Two-step delete: the first click arms the button, a second click within 3s deletes
  function requestDelete(id: string, e: Event) {
    e.stopPropagation();
    if (confirmDeleteId === id) {
      clearTimeout(confirmDeleteTimer);
      confirmDeleteId = null;
      deleteSession(id, e);
      return;
    }
    confirmDeleteId = id;
    clearTimeout(confirmDeleteTimer);
    confirmDeleteTimer = setTimeout(() => confirmDeleteId = null, 3000);
  }

  function deleteSession(id: string, e: Event) {
    e.stopPropagation();
    if (generatingIds.includes(id)) stopGeneration(id);
    sessions = sessions.filter(s => s.id !== id);
    if (sessions.length === 0) {
      createNewSession();
    } else if (currentSessionId === id) {
      currentSessionId = sessions[0].id;
    }
    saveSessions();
  }

  function parseThinking(rawContent: string) {
    let thinking = "";
    let answer = rawContent;

    const thinkMatches = rawContent.match(/<think>([\s\S]*?)<\/think>/g);
    if (thinkMatches) {
      thinking = thinkMatches
        .map(match => match.replace(/<\/?think>/g, ''))
        .join('');
      answer = rawContent.replace(/<think>[\s\S]*?<\/think>/g, '').trim();
    }

    return { thinking, answer };
  }

  // ---- RAG ----

  async function refreshDocuments() {
    try {
      documents = await invoke<DocumentInfo[]>("list_documents");
    } catch (err) {
      console.error("Failed to list documents:", err);
    }
  }

  async function handlePickAndIndexFile() {
    let selected: string | string[] | null;
    try {
      selected = await open({
        multiple: true,
        filters: [{
          name: 'Documents & Code',
          extensions: ['txt', 'md', 'rs', 'py', 'c', 'cpp', 'h', 'js', 'ts', 'json', 'svelte', 'html', 'css', 'toml', 'yaml', 'yml', 'csv', 'log', 'go', 'java', 'kt', 'sh']
        }]
      });
    } catch (err) {
      showRagStatus(`[File picker error]: ${err}`);
      return;
    }
    if (!selected) return;
    await indexPaths(Array.isArray(selected) ? selected : [selected]);
  }

  async function indexPaths(paths: string[]) {
    isIndexing = true;
    try {
      for (const filePath of paths) {
        const name = filePath.split(/[/\\]/).pop() || filePath;
        showRagStatus(`Creating embeddings: ${name}...`, false);
        showRagStatus(await invoke<string>("index_document_command", {
          provider: settings.provider,
          baseUrl: settings.baseUrl,
          apiKey: settings.apiKey,
          embedding: settings.embedding,
          filePath
        }));
      }
    } catch (err) {
      console.error("Indexing error:", err);
      showRagStatus(`[Indexing error]: ${err}`, true, true);
    } finally {
      isIndexing = false;
      await refreshDocuments();
    }
  }

  async function reindexStale() {
    const paths = documents.filter(d => isStale(d) && d.sourcePath).map(d => d.sourcePath);
    if (paths.length) await indexPaths(paths);
  }

  async function removeDocument(fileName: string) {
    try {
      await invoke("delete_document", { fileName });
    } catch (err) {
      showRagStatus(`[Delete document error]: ${err}`);
    }
    await refreshDocuments();
  }

  // ---- Chat ----

  onMount(() => {
    const unlisteners: Array<() => void> = [];
    let disposed = false;

    (async () => {
      await settings.load();
      await loadSessions();
      await refreshDocuments();

      const subs = await Promise.all([
        listen<{ sessionId: string; text: string }>("chat-chunk", (event) => {
          const sess = sessions.find(s => s.id === event.payload.sessionId);
          const last = sess?.messages[sess.messages.length - 1];
          if (sess && last?.role === "assistant") {
            last.content += event.payload.text;
            queueSaveSessions();
            if (sess.id === currentSessionId) scrollToBottom();
          }
        }),
        listen<string>("chat-done", (event) => {
          const id = event.payload;
          generatingIds = generatingIds.filter(x => x !== id);
          const sess = sessions.find(s => s.id === id);
          const last = sess?.messages[sess.messages.length - 1];
          // Dừng trước khi có token nào: bỏ bubble rỗng
          if (sess && last?.role === "assistant" && !last.content) sess.messages.pop();
          saveSessions();
        }),
        listen<string>("rag-warning", (event) => {
          showRagStatus(event.payload, true, true);
        }),
        listen<string>("rag-progress", (event) => {
          if (isIndexing) showRagStatus(`Creating embeddings ${event.payload}...`, false);
        })
      ]);

      if (disposed) subs.forEach(u => u());
      else unlisteners.push(...subs);
    })();

    return () => {
      disposed = true;
      unlisteners.forEach(u => u());
    };
  });

  async function sendMessage() {
    const userText = inputPrompt.trim();
    if (!userText || isGenerating) return;

    const sess = currentSession;
    if (!sess) return;

    inputPrompt = "";
    if (inputEl) inputEl.style.height = "auto";

    if (sess.messages.length === 0) {
      sess.title = userText.slice(0, 30) + (userText.length > 30 ? "..." : "");
    }
    sess.messages.push({ role: "user", content: userText });
    await generate(sess);
  }

  async function generate(sess: ChatSession) {
    sess.messages.push({ role: "assistant", content: "" });
    sess.updatedAt = Date.now();
    saveSessions();

    generatingIds = [...generatingIds, sess.id];
    scrollToBottom();

    // Bỏ phần <think> khỏi history và các tin nhắn assistant rỗng
    const context: Message[] = sess.messages
      .slice(0, -1)
      .map(m => m.role === "assistant" ? { role: m.role, content: parseThinking(m.content).answer } : m)
      .filter(m => m.role !== "assistant" || m.content);

    if (sess.systemPrompt) {
      context.unshift({ role: "system", content: sess.systemPrompt });
    }

    try {
      await invoke("stream_chat_command", {
        payload: {
          sessionId: sess.id,
          provider: settings.provider,
          baseUrl: settings.baseUrl,
          apiKey: settings.apiKey,
          model: settings.model,
          messages: context,
          embedding: settings.embedding
        }
      });
    } catch (err) {
      console.error(err);
      sess.messages[sess.messages.length - 1].content = `[ERROR]: ${err}`;
      generatingIds = generatingIds.filter(x => x !== sess.id);
      saveSessions();
    }
  }

  function stopGeneration(id = currentSessionId) {
    invoke("stop_chat_command", { sessionId: id }).catch(console.error);
  }

  async function regenerate() {
    const sess = currentSession;
    if (!sess || isGenerating) return;
    if (sess.messages[sess.messages.length - 1]?.role === "assistant") sess.messages.pop();
    if (sess.messages.length === 0) return;
    await generate(sess);
  }

  async function copyMessage(index: number) {
    const msg = currentMessages[index];
    try {
      await navigator.clipboard.writeText(parseThinking(msg.content).answer || msg.content);
      copiedIndex = index;
      setTimeout(() => { if (copiedIndex === index) copiedIndex = null; }, 1500);
    } catch (err) {
      console.error("Copy failed:", err);
    }
  }

  function onInputKeydown(e: KeyboardEvent) {
    // isComposing: không gửi khi đang gõ dở Telex/VNI/IME
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      sendMessage();
    }
  }

  function autoResize() {
    inputEl.style.height = "auto";
    inputEl.style.height = Math.min(inputEl.scrollHeight, 200) + "px";
  }
</script>

<div class="flex h-dvh w-screen overflow-hidden bg-[#0e0f12] text-zinc-100 font-sans antialiased">
  {#if isSidebarOpen}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div 
      onclick={() => isSidebarOpen = false} 
      class="fixed inset-0 bg-black/70 z-40 md:hidden backdrop-blur-sm transition-opacity"
    ></div>
  {/if}

  <!-- System Preset Selection Modal -->
  {#if isPresetModalOpen}
    <div class="fixed inset-0 bg-black/80 z-50 flex items-center justify-center p-4 backdrop-blur-sm">
      <div class="bg-[#13151a] border border-zinc-800 rounded-2xl w-full max-w-lg p-5 flex flex-col gap-4 shadow-2xl">
        <div class="flex items-center justify-between border-b border-zinc-800 pb-3">
          <div class="flex items-center gap-2">
            <Sparkles class="w-4 h-4 text-emerald-400" />
            <h3 class="font-bold text-sm text-zinc-100">Choose a Role (System Prompt Preset)</h3>
          </div>
          <button onclick={() => isPresetModalOpen = false} class="text-zinc-400 hover:text-white p-1">
            <X class="w-4 h-4" />
          </button>
        </div>

        <div class="flex flex-col gap-2 max-h-96 overflow-y-auto pr-1">
          {#each SYSTEM_PRESETS as preset}
            <button 
              onclick={() => applyPreset(preset)}
              class="flex flex-col items-start p-3 rounded-xl border border-zinc-800/80 bg-[#181b22] hover:bg-zinc-800/50 hover:border-emerald-500/50 text-left transition-all group cursor-pointer"
            >
              <div class="flex items-center gap-2 font-bold text-xs text-zinc-200 group-hover:text-emerald-400">
                <span>{preset.icon}</span>
                <span>{preset.name}</span>
              </div>
              <p class="text-[11px] text-zinc-400 mt-1 leading-relaxed">{preset.description}</p>
            </button>
          {/each}
        </div>
      </div>
    </div>
  {/if}

  <!-- Sidebar Config & Sessions List -->
  <aside 
    class="fixed md:static inset-y-0 left-0 z-40 w-72 bg-[#13151a] border-r border-zinc-800/50 p-4 flex flex-col gap-4 transform transition-transform duration-200 ease-out shrink-0
    {isSidebarOpen ? 'translate-x-0' : '-translate-x-full md:translate-x-0'}"
  >
    <div class="flex items-center justify-between">
      <div class="flex items-center gap-2">
        <div class="w-7 h-7 rounded-lg bg-emerald-500/10 border border-emerald-500/30 flex items-center justify-center text-emerald-400 font-bold text-sm">
          N
        </div>
        <h2 class="text-base font-bold tracking-tight text-zinc-100">NekkoChat</h2>
      </div>
      <button 
        onclick={() => isSidebarOpen = false} 
        class="md:hidden text-zinc-400 hover:text-zinc-100 p-1"
      >
        <X class="w-5 h-5" />
      </button>
    </div>

    <!-- New chat -->
    <button
      onclick={newChat}
      class="flex items-center justify-center gap-2 w-full py-2.5 px-4 bg-emerald-500 hover:bg-emerald-400 text-zinc-950 font-semibold rounded-xl text-xs transition-all shadow-md shadow-emerald-950/20 cursor-pointer shrink-0"
    >
      <Plus class="w-4 h-4" />
      <span>New chat</span>
    </button>

    <!-- Chat history: takes all remaining height, profile + settings stay pinned below -->
    <div class="flex flex-col flex-1 min-h-0">
      <div class="relative pb-2 shrink-0">
        <Search class="w-3.5 h-3.5 absolute left-3 top-2.5 text-zinc-500 pointer-events-none" />
        <input
          type="text"
          bind:value={historyQuery}
          onkeydown={(e) => e.key === "Escape" && (historyQuery = "")}
          placeholder="Search chats..."
          aria-label="Search chats"
          class="w-full bg-[#1a1d24] border border-zinc-800/80 rounded-xl pl-8 pr-8 py-2 text-xs text-zinc-100 placeholder:text-zinc-600 focus:outline-none focus:border-emerald-500/50"
        />
        {#if historyQuery}
          <button
            onclick={() => historyQuery = ""}
            title="Clear search"
            class="absolute right-2 top-2 p-0.5 text-zinc-500 hover:text-zinc-200 cursor-pointer"
          >
            <X class="w-3.5 h-3.5" />
          </button>
        {/if}
      </div>

      <div class="flex-1 min-h-0 overflow-y-auto -mr-2 pr-2">
        {#each historyGroups as group (group.label)}
          <div class="mb-2">
            <div class="sticky top-0 z-10 bg-[#13151a] text-[10px] font-semibold text-zinc-500 tracking-wider uppercase px-2 py-1">
              {group.label}
            </div>

            {#each group.items as s (s.id)}
              {@const isActive = s.id === currentSessionId}
              {@const isConfirming = confirmDeleteId === s.id}
              <div
                class="group flex items-center rounded-xl transition-colors
                {isActive ? 'bg-zinc-800/80' : 'hover:bg-zinc-800/40'}"
              >
                {#if renamingId === s.id}
                  <input
                    use:focusSelect
                    bind:value={renameDraft}
                    onkeydown={onRenameKeydown}
                    onblur={commitRename}
                    maxlength="80"
                    aria-label="Chat title"
                    class="flex-1 min-w-0 mx-1 my-1 bg-[#0e0f12] border border-emerald-500/50 rounded-lg px-2.5 py-1.5 text-xs text-zinc-100 focus:outline-none"
                  />
                {:else}
                  <button
                    onclick={() => selectSession(s.id)}
                    ondblclick={(e) => startRename(s, e)}
                    title={s.title}
                    aria-current={isActive ? "true" : undefined}
                    class="flex items-center gap-2 flex-1 min-w-0 px-3 py-2 text-left text-xs cursor-pointer
                    {isActive ? 'text-zinc-100 font-medium' : 'text-zinc-400 group-hover:text-zinc-200'}"
                  >
                    <MessageSquare class="w-3.5 h-3.5 text-zinc-500 shrink-0" />
                    <span class="truncate">{s.title}</span>
                    {#if generatingIds.includes(s.id)}
                      <Loader2 class="w-3 h-3 animate-spin text-emerald-400 shrink-0" aria-label="Generating" />
                    {/if}
                  </button>

                  <!-- Always visible on the active/armed item, otherwise on hover or keyboard focus -->
                  <div
                    class="flex items-center pr-1 shrink-0 transition-opacity
                    {isActive || isConfirming ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 focus-within:opacity-100'}"
                  >
                    {#if !isConfirming}
                      <button
                        onclick={(e) => startRename(s, e)}
                        title="Rename"
                        class="p-1.5 rounded-lg text-zinc-500 hover:text-zinc-100 hover:bg-zinc-700/50 cursor-pointer"
                      >
                        <Pencil class="w-3.5 h-3.5" />
                      </button>
                    {/if}
                    <button
                      onclick={(e) => requestDelete(s.id, e)}
                      onblur={() => confirmDeleteId === s.id && (confirmDeleteId = null)}
                      title={isConfirming ? "Click again to delete" : "Delete conversation"}
                      class="flex items-center gap-1 p-1.5 rounded-lg cursor-pointer
                      {isConfirming ? 'bg-red-500/20 text-red-300 text-[10px] font-semibold px-2' : 'text-zinc-500 hover:text-red-400 hover:bg-zinc-700/50'}"
                    >
                      <Trash2 class="w-3.5 h-3.5" />
                      {#if isConfirming}Delete?{/if}
                    </button>
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {:else}
          <p class="text-[11px] text-zinc-600 text-center px-2 py-6 leading-relaxed">
            No chats match "{historyQuery}".
          </p>
        {/each}
      </div>
    </div>

    <!-- Quick profile switch -->
    {#if settings.profiles.length > 1}
      <div class="flex flex-col gap-1">
        <label for="profile-quick-switch" class="text-[10px] font-semibold text-zinc-500 tracking-wider uppercase px-1">Profile</label>
        <select
          id="profile-quick-switch"
          bind:value={settings.activeId}
          class="w-full bg-[#1a1d24] border border-zinc-800/80 rounded-xl px-3 py-2 text-xs font-medium text-zinc-100 focus:outline-none focus:border-emerald-500/50 cursor-pointer"
        >
          {#each settings.profiles as p (p.id)}
            <option value={p.id} class="bg-[#1a1d24] text-zinc-100">{p.name || "(unnamed)"}</option>
          {/each}
        </select>
      </div>
    {/if}

    <!-- Settings entry -->
    <button
      onclick={() => goto("/settings")}
      class="flex items-center gap-3 w-full px-3 py-2.5 rounded-xl bg-[#1a1d24] hover:bg-zinc-800/70 border border-zinc-800/80 text-left transition-colors cursor-pointer"
    >
      <SettingsIcon class="w-4 h-4 text-emerald-400 shrink-0" />
      <div class="min-w-0 flex-1">
        <div class="text-xs font-semibold text-zinc-200">Settings</div>
        <div class="text-[10px] text-zinc-500 font-mono truncate">{settings.active.name} · {settings.model || "no model"}</div>
      </div>
    </button>

    <div class="pt-2 border-t border-zinc-800/40 text-[10px] text-zinc-600 text-center font-mono">
      &copy; 2026 Nekkochan
    </div>
  </aside>

  <!-- Main Chat Container -->
  <main class="flex-1 flex flex-col h-full bg-[#0e0f12] min-w-0">
    <!-- Header -->
    <header class="h-14 border-b border-zinc-800/40 px-5 flex items-center justify-between bg-[#13151a]/40 backdrop-blur-md shrink-0">
      <div class="flex items-center gap-3">
        <button 
          onclick={() => isSidebarOpen = true} 
          class="md:hidden text-zinc-400 hover:text-white p-1.5 rounded-lg bg-zinc-800/40"
        >
          <Menu class="w-5 h-5" />
        </button>
        <div class="flex items-center gap-2">
          <span class="w-2 h-2 rounded-full bg-emerald-500 shadow-[0_0_8px_rgba(16,185,129,0.5)]"></span>
          <span class="font-semibold text-xs text-zinc-200 tracking-wide">{settings.model}</span>
        </div>
      </div>

      <!-- Action Control Header -->
      <div class="flex items-center gap-2">
        <button 
          onclick={() => isPresetModalOpen = true}
          class="flex items-center gap-1.5 px-3 py-1 rounded-xl bg-zinc-800/60 hover:bg-zinc-800 text-zinc-300 border border-zinc-700/50 text-xs font-medium transition-all cursor-pointer"
          title="Choose system prompt role"
        >
          <SlidersHorizontal class="w-3.5 h-3.5 text-emerald-400" />
          <span>Role Persona</span>
        </button>

        <span class="text-[10px] font-semibold px-2.5 py-0.5 rounded-full bg-emerald-500/10 text-emerald-400 border border-emerald-500/20 font-mono uppercase">
          {settings.provider}
        </span>
      </div>
    </header>

    <!-- Chat Messages -->
    <div bind:this={chatContainer} class="flex-1 overflow-y-auto p-4 md:p-8 space-y-6">
      {#if loadError}
        <div class="max-w-xl mx-auto text-xs text-amber-300 bg-amber-500/10 border border-amber-500/30 rounded-xl px-4 py-3">
          Couldn't read previous chat history: {loadError}
        </div>
      {/if}

      {#if settings.loaded && !settings.isConfigured}
        <div class="max-w-xl mx-auto flex items-center justify-between gap-3 text-xs text-amber-300 bg-amber-500/10 border border-amber-500/30 rounded-xl px-4 py-3">
          <span>No API key set for profile "{settings.active.name}".</span>
          <button onclick={() => goto("/settings")} class="font-semibold underline underline-offset-2 hover:text-amber-200 cursor-pointer shrink-0">
            Open Settings
          </button>
        </div>
      {/if}

      {#if currentMessages.length === 0}
        <div class="h-full flex flex-col items-center justify-center text-zinc-500 px-4 text-center">
          <div class="w-12 h-12 mb-4 rounded-2xl bg-emerald-500/10 border border-emerald-500/20 flex items-center justify-center text-emerald-400 text-xl">
            ✦
          </div>
          <p class="text-sm font-semibold text-zinc-300">NekkoChat Ready</p>
          <p class="text-xs text-zinc-500 max-w-xs mt-1 leading-relaxed">Pick a Role Persona in the top right, or attach files for RAG analysis.</p>
        </div>
      {/if}

      {#each currentMessages as msg, index}
        <div class="flex flex-col {msg.role === 'user' ? 'items-end' : 'items-start'}">
          <span class="text-[10px] font-medium text-zinc-500 mb-1.5 px-1">{msg.role === 'user' ? 'You' : settings.model}</span>
          <div 
            class="max-w-[85%] md:max-w-[70%] rounded-2xl px-4 py-3 text-xs md:text-sm leading-relaxed whitespace-pre-wrap
            {msg.role === 'user' 
              ? 'bg-emerald-600 text-white rounded-tr-xs shadow-lg shadow-emerald-950/20' 
              : 'bg-[#181b22] text-zinc-200 border border-zinc-800/80 rounded-tl-xs shadow-sm'}"
          >
            {#if msg.role === 'assistant' && !msg.content && isGenerating && index === currentMessages.length - 1}
              <span class="inline-flex gap-1 items-center py-1">
                <span class="w-1.5 h-1.5 bg-emerald-400 rounded-full animate-pulse"></span>
                <span class="w-1.5 h-1.5 bg-emerald-400 rounded-full animate-pulse [animation-delay:0.2s]"></span>
                <span class="w-1.5 h-1.5 bg-emerald-400 rounded-full animate-pulse [animation-delay:0.4s]"></span>
              </span>
            {:else}
              {#if msg.role === 'assistant'}
                {@const parsed = parseThinking(msg.content)}
                
                <ThinkingBlock 
                  thinkingText={parsed.thinking} 
                  isGenerating={isGenerating && index === currentMessages.length - 1 && !parsed.answer} 
                />

                {#if parsed.answer}
                  <Markdown content={parsed.answer} />
                {/if}
              {:else}
                {msg.content}
              {/if}
            {/if}
          </div>

          {#if msg.role === 'assistant' && msg.content && !(isGenerating && index === currentMessages.length - 1)}
            <div class="flex items-center gap-1 mt-1.5 px-1">
              <button
                onclick={() => copyMessage(index)}
                title="Copy"
                class="p-1.5 rounded-lg text-zinc-500 hover:text-zinc-200 hover:bg-zinc-800/60 transition-colors cursor-pointer"
              >
                {#if copiedIndex === index}
                  <Check class="w-3.5 h-3.5 text-emerald-400" />
                {:else}
                  <Copy class="w-3.5 h-3.5" />
                {/if}
              </button>
              {#if index === currentMessages.length - 1}
                <button
                  onclick={regenerate}
                  title="Regenerate response"
                  class="p-1.5 rounded-lg text-zinc-500 hover:text-zinc-200 hover:bg-zinc-800/60 transition-colors cursor-pointer"
                >
                  <RotateCcw class="w-3.5 h-3.5" />
                </button>
              {/if}
            </div>
          {/if}
        </div>
      {/each}

      {#if lastMessage?.role === 'user' && !isGenerating}
        <div class="flex justify-start">
          <button
            onclick={regenerate}
            class="flex items-center gap-1.5 px-3 py-1.5 rounded-xl text-xs text-zinc-300 bg-zinc-800/60 hover:bg-zinc-800 border border-zinc-700/50 cursor-pointer"
          >
            <RotateCcw class="w-3.5 h-3.5 text-emerald-400" />
            Retry
          </button>
        </div>
      {/if}
    </div>

    <!-- Floating Input Box & RAG File Indexer -->
    <div class="p-4 md:p-6 bg-gradient-to-t from-[#0e0f12] via-[#0e0f12]/90 to-transparent shrink-0">
      <div class="max-w-3xl mx-auto flex flex-col gap-2">
        {#if ragStatusMsg}
          <div
            class="text-[11px] px-2.5 py-1.5 rounded-lg flex items-start gap-1.5 font-mono border break-words
            {ragStatusIsWarning
              ? 'text-amber-300 bg-amber-500/10 border-amber-500/30'
              : 'text-emerald-400 bg-emerald-500/10 border-emerald-500/30'}"
            role="status"
          >
            {#if isIndexing}
              <Loader2 class="w-3 h-3 mt-0.5 animate-spin shrink-0" />
            {:else if ragStatusIsWarning}
              <AlertTriangle class="w-3 h-3 mt-0.5 shrink-0" />
            {/if}
            <span class="min-w-0">{ragStatusMsg}</span>
          </div>
        {/if}

        {#if documents.length > 0}
          <div class="flex items-center gap-2 overflow-x-auto py-1 px-1 text-xs">
            {#each documents as doc (doc.fileName)}
              {@const stale = isStale(doc)}
              <span
                class="text-[11px] pl-2.5 pr-1 py-0.5 rounded-lg flex items-center gap-1.5 font-mono shrink-0 border
                {stale
                  ? 'text-amber-300 bg-amber-500/10 border-amber-500/30'
                  : 'text-zinc-300 bg-zinc-800/80 border-zinc-700/50'}"
                title={stale ? staleReason(doc) : `${doc.chunks} chunks · ${doc.model}`}
              >
                {#if stale}
                  <AlertTriangle class="w-3 h-3" />
                {:else}
                  <FileText class="w-3 h-3 text-emerald-400" />
                {/if}
                <span>{doc.fileName}</span>
                {#if stale && doc.sourcePath}
                  <button
                    onclick={reindexStale}
                    disabled={isIndexing}
                    title="Re-index all outdated documents"
                    class="px-1.5 py-0.5 rounded bg-amber-500/20 hover:bg-amber-500/30 cursor-pointer disabled:opacity-50"
                  >
                    Re-index
                  </button>
                {/if}
                <button
                  onclick={() => removeDocument(doc.fileName)}
                  title="Remove from RAG"
                  class="p-0.5 rounded hover:text-red-400 hover:bg-zinc-700/60 cursor-pointer"
                >
                  <X class="w-3 h-3" />
                </button>
              </span>
            {/each}
          </div>
        {/if}

        <form 
          onsubmit={(e) => { e.preventDefault(); sendMessage(); }} 
          class="relative flex items-end bg-[#181b22] border border-zinc-800/80 rounded-2xl shadow-xl shadow-black/40 focus-within:border-emerald-500/50 focus-within:ring-1 focus-within:ring-emerald-500/20 transition-all p-1.5 gap-2"
        >
          <button
            type="button"
            onclick={handlePickAndIndexFile}
            disabled={isIndexing}
            title="Attach documents / source files for RAG vector search"
            class="p-2.5 text-zinc-400 hover:text-emerald-400 hover:bg-zinc-800/60 rounded-xl transition-all shrink-0 disabled:opacity-50 cursor-pointer"
          >
            {#if isIndexing}
              <Loader2 class="w-4 h-4 animate-spin text-emerald-400" />
            {:else}
              <Paperclip class="w-4 h-4" />
            {/if}
          </button>

          <textarea
            bind:this={inputEl}
            bind:value={inputPrompt}
            oninput={autoResize}
            onkeydown={onInputKeydown}
            rows="1"
            placeholder={isGenerating ? "AI is responding..." : "Ask anything or have it analyze your documents... (Shift+Enter for new line)"}
            class="w-full bg-transparent px-2 py-2.5 text-xs md:text-sm text-zinc-100 placeholder:text-zinc-500 focus:outline-none resize-none max-h-[200px]"
          ></textarea>

          {#if isGenerating}
            <button
              type="button"
              onclick={() => stopGeneration()}
              title="Stop"
              class="bg-red-500/90 hover:bg-red-400 text-zinc-950 font-bold px-4 py-2.5 rounded-xl text-xs transition-all shrink-0 cursor-pointer flex items-center justify-center"
            >
              <Square class="w-4 h-4 fill-current" />
            </button>
          {:else}
            <button
              type="submit"
              disabled={!inputPrompt.trim()}
              class="bg-emerald-500 hover:bg-emerald-400 disabled:bg-zinc-800/50 disabled:text-zinc-600 text-zinc-950 font-bold px-4 py-2.5 rounded-xl text-xs transition-all shrink-0 cursor-pointer disabled:cursor-not-allowed flex items-center justify-center"
            >
              <Send class="w-4 h-4" />
            </button>
          {/if}
        </form>
      </div>
    </div>
  </main>
</div>