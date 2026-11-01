import { invoke } from "@tauri-apps/api/core";

export type EmbeddingConfig = { baseUrl: string; apiKey: string; model: string };

/** A named connection config. Every field is free text entered by the user. */
export type Profile = {
  id: string;
  name: string;
  provider: string;
  baseUrl: string;
  apiKey: string;
  model: string;
};

type StoredSettings = {
  activeProfileId: string;
  profiles: Profile[];
  embedding: EmbeddingConfig;
  // Legacy single-provider fields, only present in settings.json written by older versions
  provider?: string;
  baseUrl?: string;
  apiKey?: string;
  model?: string;
};

export type SaveState = "idle" | "pending" | "saving" | "saved" | "error";

export const PROVIDERS = [
  { id: "ollama", label: "Ollama (Local)", baseUrl: "http://localhost:11434" },
  { id: "openai", label: "OpenAI Compatible", baseUrl: "https://api.openai.com/v1" },
  { id: "anthropic", label: "Anthropic Compatible", baseUrl: "https://api.anthropic.com/v1" }
] as const;

const SAVE_DEBOUNCE_MS = 500;

function newProfile(overrides: Partial<Profile> = {}): Profile {
  return {
    id: crypto.randomUUID(),
    name: "New profile",
    provider: "openai",
    baseUrl: "https://api.openai.com/v1",
    apiKey: "",
    model: "",
    ...overrides
  };
}

function defaultProfile(): Profile {
  return newProfile({
    name: "NVIDIA",
    baseUrl: "https://integrate.api.nvidia.com/v1",
    model: "nvidia/nemotron-3-ultra-550b-a55b"
  });
}

class SettingsStore {
  profiles = $state<Profile[]>([defaultProfile()]);
  activeId = $state("");
  embedBaseUrl = $state("");
  embedApiKey = $state("");
  embedModel = $state("");

  loaded = $state(false);
  saveState = $state<SaveState>("idle");
  saveError = $state("");

  #loading: Promise<void> | null = null;
  #savedTimer: ReturnType<typeof setTimeout> | undefined;

  constructor() {
    this.activeId = this.profiles[0].id;

    // Module-level singleton, so $effect.root is needed. Auto-saves (debounced) on every edit.
    // The timer lives in the effect, so it keeps running when the user leaves the Settings page.
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

  /** The profile currently used for chat. Always defined: there is at least one profile. */
  get active(): Profile {
    return this.profiles.find((p) => p.id === this.activeId) ?? this.profiles[0];
  }

  get provider() {
    return this.active.provider;
  }
  get baseUrl() {
    return this.active.baseUrl;
  }
  get apiKey() {
    return this.active.apiKey;
  }
  get model() {
    return this.active.model;
  }

  get embedding(): EmbeddingConfig {
    return { baseUrl: this.embedBaseUrl.trim(), apiKey: this.embedApiKey, model: this.embedModel.trim() };
  }

  get isConfigured() {
    return this.provider === "ollama" || this.apiKey.trim().length > 0;
  }

  /** Loads once, however many pages call it. */
  load(): Promise<void> {
    this.#loading ??= this.#doLoad();
    return this.#loading;
  }

  async #doLoad() {
    try {
      const saved = await invoke<StoredSettings | null>("load_settings");
      if (saved) {
        if (saved.profiles?.length) {
          this.profiles = saved.profiles;
          this.activeId = saved.profiles.some((p) => p.id === saved.activeProfileId)
            ? saved.activeProfileId
            : saved.profiles[0].id;
        } else if (saved.provider) {
          // Migrate the old single-provider format into one profile
          const label = PROVIDERS.find((p) => p.id === saved.provider)?.label ?? "Default";
          const migrated = newProfile({
            name: label,
            provider: saved.provider,
            baseUrl: saved.baseUrl ?? "",
            apiKey: saved.apiKey ?? "",
            model: saved.model ?? ""
          });
          this.profiles = [migrated];
          this.activeId = migrated.id;
        }
        this.embedBaseUrl = saved.embedding?.baseUrl ?? "";
        this.embedApiKey = saved.embedding?.apiKey ?? "";
        this.embedModel = saved.embedding?.model ?? "";
      }
    } catch (err) {
      // Keep loaded=true: the user can still work with the defaults
      console.error("Failed to load settings:", err);
    } finally {
      this.loaded = true;
    }
  }

  addProfile() {
    const profile = newProfile({ name: this.#uniqueName("New profile") });
    this.profiles.push(profile);
    this.activeId = profile.id;
  }

  duplicateActive() {
    const src = $state.snapshot(this.active);
    const copy = newProfile({ ...src, id: crypto.randomUUID(), name: this.#uniqueName(`${src.name} copy`) });
    this.profiles.push(copy);
    this.activeId = copy.id;
  }

  /** Deleting the last remaining profile is not allowed. */
  deleteActive() {
    if (this.profiles.length <= 1) return;
    const idx = this.profiles.findIndex((p) => p.id === this.activeId);
    this.profiles.splice(idx, 1);
    this.activeId = this.profiles[Math.max(0, idx - 1)].id;
  }

  /** Changes the provider type and updates the Base URL, unless the user entered a custom one. */
  setProvider(id: string) {
    const profile = this.active;
    const isDefaultUrl = PROVIDERS.some((p) => p.baseUrl === profile.baseUrl);
    profile.provider = id;
    const preset = PROVIDERS.find((p) => p.id === id);
    if (preset && (!profile.baseUrl.trim() || isDefaultUrl)) profile.baseUrl = preset.baseUrl;
  }

  resetBaseUrl() {
    const preset = PROVIDERS.find((p) => p.id === this.active.provider);
    if (preset) this.active.baseUrl = preset.baseUrl;
  }

  #uniqueName(base: string) {
    const names = new Set(this.profiles.map((p) => p.name));
    if (!names.has(base)) return base;
    let i = 2;
    while (names.has(`${base} ${i}`)) i++;
    return `${base} ${i}`;
  }

  #snapshot(): StoredSettings {
    return {
      activeProfileId: this.activeId,
      profiles: $state.snapshot(this.profiles),
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
