import { invoke } from "@tauri-apps/api/core";

export type EmbeddingConfig = { baseUrl: string; apiKey: string; model: string };

type StoredSettings = {
  provider: string;
  baseUrl: string;
  apiKey: string;
  model: string;
  embedding: EmbeddingConfig;
};

export type SaveState = "idle" | "pending" | "saving" | "saved" | "error";

export const PROVIDERS = [
  { id: "ollama", label: "Ollama (Local)", baseUrl: "http://localhost:11434" },
  { id: "openai", label: "OpenAI Compatible", baseUrl: "https://api.openai.com/v1" },
  { id: "anthropic", label: "Anthropic Compatible", baseUrl: "https://api.anthropic.com/v1" }
] as const;

const SAVE_DEBOUNCE_MS = 500;

class SettingsStore {
  provider = $state("openai");
  baseUrl = $state("https://integrate.api.nvidia.com/v1");
  apiKey = $state("");
  model = $state("nvidia/nemotron-3-ultra-550b-a55b");
  embedBaseUrl = $state("");
  embedApiKey = $state("");
  embedModel = $state("");

  loaded = $state(false);
  saveState = $state<SaveState>("idle");
  saveError = $state("");

  #loading: Promise<void> | null = null;
  #savedTimer: ReturnType<typeof setTimeout> | undefined;

  constructor() {
    // Module-level singleton nên cần $effect.root; tự lưu (debounce) mỗi khi user chỉnh.
    // Timer nằm trong effect nên vẫn chạy tiếp khi user rời trang Settings.
    $effect.root(() => {
      $effect(() => {
        const snapshot = this.#snapshot();
        if (!this.loaded) return;
        this.saveState = "pending";
        const t = setTimeout(() => this.#save(snapshot), SAVE_DEBOUNCE_MS);
        return () => clearTimeout(t);
      });
    });
  }

  get embedding(): EmbeddingConfig {
    return { baseUrl: this.embedBaseUrl.trim(), apiKey: this.embedApiKey, model: this.embedModel.trim() };
  }

  get isConfigured() {
    return this.provider === "ollama" || this.apiKey.trim().length > 0;
  }

  /** Chỉ load một lần dù nhiều trang cùng gọi. */
  load(): Promise<void> {
    this.#loading ??= this.#doLoad();
    return this.#loading;
  }

  async #doLoad() {
    try {
      const saved = await invoke<StoredSettings | null>("load_settings");
      if (saved) {
        this.provider = saved.provider;
        this.baseUrl = saved.baseUrl;
        this.apiKey = saved.apiKey;
        this.model = saved.model;
        this.embedBaseUrl = saved.embedding?.baseUrl ?? "";
        this.embedApiKey = saved.embedding?.apiKey ?? "";
        this.embedModel = saved.embedding?.model ?? "";
      }
    } catch (err) {
      // Không đánh dấu loaded=false: user vẫn dùng được với giá trị mặc định
      console.error("Failed to load settings:", err);
    } finally {
      this.loaded = true;
    }
  }

  /** Đổi provider và đổi luôn Base URL, nhưng không ghi đè URL tùy chỉnh của user. */
  setProvider(id: string) {
    const isDefaultUrl = PROVIDERS.some((p) => p.baseUrl === this.baseUrl);
    this.provider = id;
    const preset = PROVIDERS.find((p) => p.id === id);
    if (preset && (!this.baseUrl.trim() || isDefaultUrl)) this.baseUrl = preset.baseUrl;
  }

  resetBaseUrl() {
    const preset = PROVIDERS.find((p) => p.id === this.provider);
    if (preset) this.baseUrl = preset.baseUrl;
  }

  #snapshot(): StoredSettings {
    return {
      provider: this.provider,
      baseUrl: this.baseUrl,
      apiKey: this.apiKey,
      model: this.model,
      embedding: { baseUrl: this.embedBaseUrl, apiKey: this.embedApiKey, model: this.embedModel }
    };
  }

  async #save(snapshot: StoredSettings) {
    this.saveState = "saving";
    try {
      await invoke("save_settings", { settings: snapshot });
      this.saveState = "saved";
      this.saveError = "";
      clearTimeout(this.#savedTimer);
      this.#savedTimer = setTimeout(() => {
        if (this.saveState === "saved") this.saveState = "idle";
      }, 2500);
    } catch (err) {
      console.error("Failed to save settings:", err);
      this.saveError = String(err);
      this.saveState = "error";
    }
  }
}

export const settings = new SettingsStore();
