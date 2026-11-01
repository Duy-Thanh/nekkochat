import { invoke } from "@tauri-apps/api/core";

/**
 * API keys never live in this file's state. They are sent to Rust once (`setApiKey`) and stay
 * there: the frontend only knows whether a key exists (`hasApiKey`) and which origin it is bound
 * to (`keyOrigin`). A compromised page therefore has no key to read.
 */
export type EmbeddingConfig = { baseUrl: string; model: string };

/** A named connection config. Every field is free text entered by the user. */
export type Profile = {
  id: string;
  name: string;
  provider: string;
  baseUrl: string;
  model: string;
  /** Derived from secure storage by Rust */
  hasApiKey: boolean;
  /** The origin the saved key is bound to; the key is only ever sent there */
  keyOrigin: string;
};

type KeyInfo = { hasApiKey: boolean; keyOrigin: string };

type StoredSettings = {
  activeProfileId: string;
  profiles: Profile[];
  embedding: EmbeddingConfig & Partial<KeyInfo>;
  // Legacy single-provider fields, only present in settings.json written by older versions
  provider?: string;
  baseUrl?: string;
  model?: string;
};

/**
 * Where API keys are kept. "keychain" is the OS credential store. When there is none, an
 * encrypted vault protected by a master password is used instead.
 */
export type SecretsStatus = "keychain" | "vault-unlocked" | "vault-locked" | "vault-missing";

export type SaveState = "idle" | "pending" | "saving" | "saved" | "error";

export const PROVIDERS = [
  { id: "ollama", label: "Ollama (Local)", baseUrl: "http://localhost:11434" },
  { id: "openai", label: "OpenAI Compatible", baseUrl: "https://api.openai.com/v1" },
  { id: "anthropic", label: "Anthropic Compatible", baseUrl: "https://api.anthropic.com/v1" }
] as const;

const SAVE_DEBOUNCE_MS = 500;

/** Same normalisation Rust uses for the origin a key is bound to; empty if the URL is invalid. */
export function originOf(url: string): string {
  try {
    return new URL(url.trim()).origin;
  } catch {
    return "";
  }
}

function newProfile(overrides: Partial<Profile> = {}): Profile {
  return {
    id: crypto.randomUUID(),
    name: "New profile",
    provider: "openai",
    baseUrl: "https://api.openai.com/v1",
    model: "",
    hasApiKey: false,
    keyOrigin: "",
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
  embedModel = $state("");
  embedHasApiKey = $state(false);
  embedKeyOrigin = $state("");

  loaded = $state(false);
  saveState = $state<SaveState>("idle");
  saveError = $state("");
  /** Warnings from secure storage (a key that could not be read, a legacy key left behind...). */
  keychainWarning = $state("");
  secretsStatus = $state<SecretsStatus>("keychain");
  /** The user chose "continue without keys" on the unlock prompt for this session. */
  unlockDismissed = $state(false);
  /**
   * Bumped whenever the vault is created, unlocked, locked or reset. Chat history and documents
   * are encrypted with the same secure storage, so the pages reload them when this changes.
   */
  dataEpoch = $state(0);

  #loading: Promise<void> | null = null;
  #savedTimer: ReturnType<typeof setTimeout> | undefined;
  #saving = false;
  #saveQueued = false;

  constructor() {
    this.activeId = this.profiles[0].id;

    // Module-level singleton, so $effect.root is needed. Auto-saves (debounced) on every edit.
    // The timer lives in the effect, so it keeps running when the user leaves the Settings page.
    $effect.root(() => {
      $effect(() => {
        this.#snapshot(); // subscribe to every saved field
        if (!this.loaded) return;
        this.saveState = "pending";
        const t = setTimeout(() => this.#save(), SAVE_DEBOUNCE_MS);
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
  get model() {
    return this.active.model;
  }

  get embedding(): EmbeddingConfig {
    return { baseUrl: this.embedBaseUrl.trim(), model: this.embedModel.trim() };
  }

  get isConfigured() {
    return this.provider === "ollama" || this.active.hasApiKey;
  }

  /** Loads once, however many pages call it. */
  load(): Promise<void> {
    this.#loading ??= this.#doLoad();
    return this.#loading;
  }

  async #doLoad() {
    try {
      const result = await invoke<{
        settings: StoredSettings;
        warnings: string[];
        secrets: SecretsStatus;
      } | null>("load_settings");
      if (!result) this.secretsStatus = await invoke<SecretsStatus>("secrets_status");
      if (result) {
        const saved = result.settings;
        this.keychainWarning = result.warnings.join(" ");
        this.secretsStatus = result.secrets;
        if (saved.profiles?.length) {
          this.profiles = saved.profiles.map((p) => ({ ...newProfile(), ...p }));
          this.activeId = saved.profiles.some((p) => p.id === saved.activeProfileId)
            ? saved.activeProfileId
            : saved.profiles[0].id;
        } else if (saved.provider) {
          // Migrate the old single-provider format into one profile (its key is not carried over)
          const label = PROVIDERS.find((p) => p.id === saved.provider)?.label ?? "Default";
          const migrated = newProfile({
            name: label,
            provider: saved.provider,
            baseUrl: saved.baseUrl ?? "",
            model: saved.model ?? ""
          });
          this.profiles = [migrated];
          this.activeId = migrated.id;
        }
        this.embedBaseUrl = saved.embedding?.baseUrl ?? "";
        this.embedModel = saved.embedding?.model ?? "";
        this.embedHasApiKey = saved.embedding?.hasApiKey ?? false;
        this.embedKeyOrigin = saved.embedding?.keyOrigin ?? "";
      }
    } catch (err) {
      // Keep loaded=true: the user can still work with the defaults
      console.error("Failed to load settings:", err);
    } finally {
      this.loaded = true;
    }
  }

  // ---- API keys: write-only ----

  /** Saves a key for a profile, bound to that profile's current Base URL. */
  async setApiKey(profileId: string, apiKey: string) {
    const profile = this.profiles.find((p) => p.id === profileId);
    if (!profile) throw "Unknown profile";
    const info = await invoke<KeyInfo>("set_api_key", { profileId, apiKey, baseUrl: profile.baseUrl });
    profile.hasApiKey = info.hasApiKey;
    profile.keyOrigin = info.keyOrigin;
  }

  async clearApiKey(profileId: string) {
    const profile = this.profiles.find((p) => p.id === profileId);
    const info = await invoke<KeyInfo>("clear_api_key", { profileId });
    if (profile) {
      profile.hasApiKey = info.hasApiKey;
      profile.keyOrigin = info.keyOrigin;
    }
  }

  async setEmbeddingKey(apiKey: string) {
    const info = await invoke<KeyInfo>("set_embedding_key", { apiKey, baseUrl: this.embedBaseUrl });
    this.embedHasApiKey = info.hasApiKey;
    this.embedKeyOrigin = info.keyOrigin;
  }

  async clearEmbeddingKey() {
    const info = await invoke<KeyInfo>("clear_embedding_key");
    this.embedHasApiKey = info.hasApiKey;
    this.embedKeyOrigin = info.keyOrigin;
  }

  // ---- Secure storage (OS keychain, or the encrypted vault when there is none) ----

  async createVault(passphrase: string) {
    this.secretsStatus = await invoke<SecretsStatus>("vault_create", { passphrase });
    this.dataEpoch++;
  }

  async unlockVault(passphrase: string) {
    this.secretsStatus = await invoke<SecretsStatus>("vault_unlock", { passphrase });
    await this.#refreshKeyInfo();
    this.dataEpoch++;
  }

  /** Locks the vault. Rust forgets the decrypted keys, so no key is usable until unlock. */
  async lockVault() {
    this.secretsStatus = await invoke<SecretsStatus>("vault_lock");
    this.#markKeysUnavailable();
    this.unlockDismissed = false;
    this.dataEpoch++;
  }

  async changePassphrase(oldPassphrase: string, newPassphrase: string) {
    this.secretsStatus = await invoke<SecretsStatus>("vault_change_passphrase", {
      old: oldPassphrase,
      new: newPassphrase
    });
  }

  /** Deletes the vault and every key in it (for a forgotten master password). */
  async resetVault() {
    this.secretsStatus = await invoke<SecretsStatus>("vault_reset");
    this.#markKeysUnavailable();
    this.dataEpoch++;
  }

  #markKeysUnavailable() {
    for (const p of this.profiles) {
      p.hasApiKey = false;
      p.keyOrigin = "";
    }
    this.embedHasApiKey = false;
    this.embedKeyOrigin = "";
  }

  /** Re-reads which keys exist (not the keys themselves) after the vault changes state. */
  async #refreshKeyInfo() {
    const result = await invoke<{ settings: StoredSettings; warnings: string[] } | null>("load_settings");
    if (!result) return;
    this.keychainWarning = result.warnings.join(" ");
    for (const stored of result.settings.profiles ?? []) {
      const local = this.profiles.find((p) => p.id === stored.id);
      if (local) {
        local.hasApiKey = stored.hasApiKey ?? false;
        local.keyOrigin = stored.keyOrigin ?? "";
      }
    }
    this.embedHasApiKey = result.settings.embedding?.hasApiKey ?? false;
    this.embedKeyOrigin = result.settings.embedding?.keyOrigin ?? "";
  }

  // ---- Profiles ----

  addProfile() {
    const profile = newProfile({ name: this.#uniqueName("New profile") });
    this.profiles.push(profile);
    this.activeId = profile.id;
  }

  /** Copies everything except the key: keys are bound to an endpoint and are never copied around. */
  duplicateActive() {
    const src = $state.snapshot(this.active);
    const copy = newProfile({
      ...src,
      id: crypto.randomUUID(),
      name: this.#uniqueName(`${src.name} copy`),
      hasApiKey: false,
      keyOrigin: ""
    });
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

  #snapshot() {
    return {
      activeProfileId: this.activeId,
      profiles: $state.snapshot(this.profiles),
      embedding: { baseUrl: this.embedBaseUrl, model: this.embedModel }
    };
  }

  /** One save at a time, always the latest state, so an older snapshot can never overwrite a newer one. */
  async #save() {
    if (this.#saving) {
      this.#saveQueued = true;
      return;
    }
    this.#saving = true;
    this.saveState = "saving";
    try {
      do {
        this.#saveQueued = false;
        await invoke("save_settings", { settings: this.#snapshot() });
      } while (this.#saveQueued);
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
    } finally {
      this.#saving = false;
    }
  }
}

export const settings = new SettingsStore();
