<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { onMount } from "svelte";
  import Markdown from '$lib/Markdown.svelte';
  import ThinkingBlock from '$lib/ThinkingBlock.svelte';

  import { 
    X, 
    Menu, 
    ChevronDown, 
    Send, 
    Plus,
    MessageSquare,
    Trash2
  } from '@lucide/svelte';

  type Message = {
    role: "user" | "assistant" | "system";
    content: string;
  };

  type ChatSession = {
    id: string;
    title: string;
    updatedAt: number;
    messages: Message[];
  };

  let provider = $state("openai");
  let baseUrl = $state("https://integrate.api.nvidia.com/v1");
  let apiKey = $state("");
  let selectedModel = $state("nvidia/nemotron-3-ultra-550b-a55b");
  let inputPrompt = $state("");
  
  // Sessions State
  let sessions = $state<ChatSession[]>([]);
  let currentSessionId = $state<string>("");

  let isGenerating = $state(false);
  let isSidebarOpen = $state(false);

  let chatContainer: HTMLDivElement;

  // Lấy danh sách tin nhắn của Session hiện tại
  let currentMessages = $derived.by(() => {
    const s = sessions.find(s => s.id === currentSessionId);
    return s ? s.messages : [];
  });

  $effect(() => {
    if (provider === "ollama") {
      baseUrl = "http://localhost:11434";
    } else if (provider === "openai") {
      baseUrl = "https://api.openai.com/v1";
    } else if (provider === "anthropic") {
      baseUrl = "https://api.anthropic.com/v1";
    }
  });

  function scrollToBottom() {
    setTimeout(() => {
      if (chatContainer) chatContainer.scrollTop = chatContainer.scrollHeight;
    }, 50);
  }

  // Load / Save Sessions vào localStorage
  async function loadSessions() {
    try {
      const savedSessions = await invoke<ChatSession[]>("load_sessions");
      if (savedSessions && savedSessions.length > 0) {
        sessions = savedSessions;
        currentSessionId = sessions[0].id;
      } else {
        createNewSession();
      }
    } catch (err) {
      console.error("Lỗi load sessions từ Rust:", err);
      createNewSession();
    }
  }

  // Save Sessions sang Rust Persistence File
  async function saveSessions() {
    try {
      await invoke("save_sessions", { sessions: $state.snapshot(sessions) });
    } catch (err) {
      console.error("Lỗi save sessions sang Rust:", err);
    }
  }

  function createNewSession() {
    const newSession: ChatSession = {
      id: crypto.randomUUID(),
      title: "New Chat",
      updatedAt: Date.now(),
      messages: []
    };
    sessions = [newSession, ...sessions];
    currentSessionId = newSession.id;
    saveSessions();
  }

  function deleteSession(id: string, e: Event) {
    e.stopPropagation();
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

  onMount(() => {
    loadSessions();

    let unlistenChunk: (() => void) | undefined;
    let unlistenDone: (() => void) | undefined;

    (async () => {
      unlistenChunk = await listen<string>("chat-chunk", (event) => {
        const sessionIndex = sessions.findIndex(s => s.id === currentSessionId);
        if (sessionIndex !== -1) {
          const msgs = sessions[sessionIndex].messages;
          if (msgs.length > 0 && msgs[msgs.length - 1].role === "assistant") {
            msgs[msgs.length - 1].content += event.payload;
            saveSessions();
          }
        }
        scrollToBottom();
      });

      unlistenDone = await listen("chat-done", () => {
        isGenerating = false;
        saveSessions();
      });
    })();

    return () => {
      if (unlistenChunk) unlistenChunk();
      if (unlistenDone) unlistenDone();
    };
  });

  async function sendMessage() {
    if (!inputPrompt.trim() || isGenerating) return;

    const userText = inputPrompt;
    inputPrompt = "";

    const sessionIndex = sessions.findIndex(s => s.id === currentSessionId);
    if (sessionIndex === -1) return;

    const currentSess = sessions[sessionIndex];

    // Cập nhật Tên Session nếu đây là tin nhắn đầu tiên
    if (currentSess.messages.length === 0) {
      currentSess.title = userText.slice(0, 30) + (userText.length > 30 ? "..." : "");
    }

    // Push tin nhắn User + Assistant trống
    currentSess.messages.push({ role: "user", content: userText });
    currentSess.messages.push({ role: "assistant", content: "" });
    currentSess.updatedAt = Date.now();
    saveSessions();

    isGenerating = true;
    scrollToBottom();

    // Lọc bỏ bớt tag <think>...</think> ở các tin nhắn cũ trước khi gửi context cho LLM
    const cleanContextMessages = currentSess.messages.slice(0, -1).map(m => {
      if (m.role === "assistant") {
        const parsed = parseThinking(m.content);
        return { role: m.role, content: parsed.answer };
      }
      return m;
    });

    try {
      await invoke("stream_chat_command", {
        payload: {
          provider,
          baseUrl,
          apiKey,
          model: selectedModel,
          messages: cleanContextMessages
        }
      });
    } catch (err) {
      console.error(err);
      const msgs = currentSess.messages;
      msgs[msgs.length - 1].content = `[ERROR]: ${err}`;
      isGenerating = false;
      saveSessions();
    }
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

  <!-- Sidebar Config & Sessions List -->
  <aside 
    class="fixed md:static inset-y-0 left-0 z-50 w-72 bg-[#13151a] border-r border-zinc-800/50 p-4 flex flex-col gap-4 transform transition-transform duration-200 ease-out shrink-0
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

    <!-- Nút New Chat -->
    <button 
      onclick={createNewSession}
      class="flex items-center justify-center gap-2 w-full py-2.5 px-4 bg-emerald-500 hover:bg-emerald-400 text-zinc-950 font-semibold rounded-xl text-xs transition-all shadow-md shadow-emerald-950/20 cursor-pointer"
    >
      <Plus class="w-4 h-4" />
      <span>Cuộc trò chuyện mới</span>
    </button>

    <!-- Sessions List (History) -->
    <div class="flex flex-col gap-1 overflow-y-auto max-h-48 border-b border-zinc-800/60 pb-3 pr-1">
      <span class="text-[10px] font-semibold text-zinc-500 tracking-wider uppercase px-1 mb-1">Lịch sử Chat</span>
      {#each sessions as s (s.id)}
        <div 
          onclick={() => currentSessionId = s.id}
          role="button"
          tabindex="0"
          onkeydown={(e) => e.key === 'Enter' && (currentSessionId = s.id)}
          class="flex items-center justify-between px-3 py-2 rounded-xl text-xs cursor-pointer transition-colors group
          {s.id === currentSessionId ? 'bg-zinc-800/80 text-zinc-100 font-medium' : 'text-zinc-400 hover:bg-zinc-800/40 hover:text-zinc-200'}"
        >
          <div class="flex items-center gap-2 truncate pr-2">
            <MessageSquare class="w-3.5 h-3.5 text-zinc-500 shrink-0" />
            <span class="truncate">{s.title}</span>
          </div>
          <button 
            onclick={(e) => deleteSession(s.id, e)}
            class="opacity-0 group-hover:opacity-100 hover:text-red-400 p-1 transition-opacity"
            title="Xóa conversation"
          >
            <Trash2 class="w-3.5 h-3.5" />
          </button>
        </div>
      {/each}
    </div>

    <!-- Provider Config Controls -->
    <div class="flex flex-col gap-3.5 overflow-y-auto flex-1 pr-1 pt-1">
      <div class="flex flex-col gap-1">
        <label for="provider-select" class="text-[10px] font-semibold text-zinc-400 tracking-wider uppercase">Provider</label>
        <div class="relative">
          <select 
            id="provider-select" 
            bind:value={provider} 
            class="w-full bg-[#1a1d24] border border-zinc-800/80 rounded-xl px-3 py-2 text-xs font-medium text-zinc-100 appearance-none focus:outline-none focus:border-emerald-500/50 focus:ring-1 focus:ring-emerald-500/20 transition-all cursor-pointer"
          >
            <option value="ollama" class="bg-[#1a1d24] text-zinc-100">Ollama (Local)</option>
            <option value="openai" class="bg-[#1a1d24] text-zinc-100">OpenAI Compatible</option>
            <option value="anthropic" class="bg-[#1a1d24] text-zinc-100">Anthropic Compatible</option>
          </select>
          <div class="pointer-events-none absolute inset-y-0 right-0 flex items-center px-3 text-zinc-400 text-xs">
            <ChevronDown class="w-3.5 h-3.5" />
          </div>
        </div>
      </div>

      <div class="flex flex-col gap-1">
        <label for="base-url-input" class="text-[10px] font-semibold text-zinc-400 tracking-wider uppercase">Base URL</label>
        <input 
          id="base-url-input" 
          type="text" 
          bind:value={baseUrl} 
          class="bg-[#1a1d24] border border-zinc-800/80 rounded-xl px-3 py-1.5 text-xs font-mono text-zinc-200 placeholder:text-zinc-600 focus:outline-none focus:border-emerald-500/50 focus:ring-1 focus:ring-emerald-500/20 transition-all" 
        />
      </div>

      {#if provider !== 'ollama'}
        <div class="flex flex-col gap-1">
          <label for="api-key-input" class="text-[10px] font-semibold text-zinc-400 tracking-wider uppercase">API Key</label>
          <input 
            id="api-key-input" 
            type="password" 
            bind:value={apiKey} 
            placeholder="sk-..." 
            class="bg-[#1a1d24] border border-zinc-800/80 rounded-xl px-3 py-1.5 text-xs font-mono text-zinc-200 placeholder:text-zinc-600 focus:outline-none focus:border-emerald-500/50 focus:ring-1 focus:ring-emerald-500/20 transition-all" 
          />
        </div>
      {/if}

      <div class="flex flex-col gap-1">
        <label for="model-name-input" class="text-[10px] font-semibold text-zinc-400 tracking-wider uppercase">Model Name</label>
        <input 
          id="model-name-input" 
          type="text" 
          bind:value={selectedModel} 
          class="bg-[#1a1d24] border border-zinc-800/80 rounded-xl px-3 py-1.5 text-xs text-zinc-200 placeholder:text-zinc-600 focus:outline-none focus:border-emerald-500/50 focus:ring-1 focus:ring-emerald-500/20 transition-all" 
        />
      </div>
    </div>

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
          <span class="font-semibold text-xs text-zinc-200 tracking-wide">{selectedModel}</span>
        </div>
      </div>
      <span class="text-[10px] font-semibold px-2.5 py-0.5 rounded-full bg-emerald-500/10 text-emerald-400 border border-emerald-500/20 font-mono uppercase">
        {provider}
      </span>
    </header>

    <!-- Chat Messages -->
    <div bind:this={chatContainer} class="flex-1 overflow-y-auto p-4 md:p-8 space-y-6">
      {#if currentMessages.length === 0}
        <div class="h-full flex flex-col items-center justify-center text-zinc-500 px-4 text-center">
          <div class="w-12 h-12 mb-4 rounded-2xl bg-emerald-500/10 border border-emerald-500/20 flex items-center justify-center text-emerald-400 text-xl">
            ✦
          </div>
          <p class="text-sm font-semibold text-zinc-300">NekkoChat Session Ready</p>
          <p class="text-xs text-zinc-500 max-w-xs mt-1 leading-relaxed">Hỏi câu đầu tiên để khởi tạo chủ đề chat nhé mày!</p>
        </div>
      {/if}

      {#each currentMessages as msg, index}
        <div class="flex flex-col {msg.role === 'user' ? 'items-end' : 'items-start'}">
          <span class="text-[10px] font-medium text-zinc-500 mb-1.5 px-1">{msg.role === 'user' ? 'You' : selectedModel}</span>
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
        </div>
      {/each}
    </div>

    <!-- Floating Input Box -->
    <div class="p-4 md:p-6 bg-gradient-to-t from-[#0e0f12] via-[#0e0f12]/90 to-transparent shrink-0">
      <form 
        onsubmit={(e) => { e.preventDefault(); sendMessage(); }} 
        class="max-w-3xl mx-auto relative flex items-center bg-[#181b22] border border-zinc-800/80 rounded-2xl shadow-xl shadow-black/40 focus-within:border-emerald-500/50 focus-within:ring-1 focus-within:ring-emerald-500/20 transition-all p-1.5"
      >
        <input
          type="text"
          bind:value={inputPrompt}
          disabled={isGenerating}
          placeholder={isGenerating ? "AI đang trả lời..." : "Hỏi bất cứ điều gì..."}
          class="w-full bg-transparent px-4 py-2.5 text-xs md:text-sm text-zinc-100 placeholder:text-zinc-500 focus:outline-none disabled:opacity-50"
        />
        <button
          type="submit"
          disabled={isGenerating || !inputPrompt.trim()}
          class="bg-emerald-500 hover:bg-emerald-400 disabled:bg-zinc-800/50 disabled:text-zinc-600 text-zinc-950 font-bold px-4 py-2.5 rounded-xl text-xs transition-all shrink-0 cursor-pointer disabled:cursor-not-allowed flex items-center justify-center"
        >
          <Send class="w-4 h-4" />
        </button>
      </form>
    </div>
  </main>
</div>