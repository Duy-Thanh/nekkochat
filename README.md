# NekkoChat

A desktop and mobile chat client for LLMs, built with Tauri 2, SvelteKit and Rust. It runs on Windows, Linux, macOS, Android and iOS.

## Features

- **Several providers:** OpenAI-compatible APIs (OpenAI, NVIDIA, OpenRouter, ...), Anthropic, and local models through Ollama. Each connection profile has its own provider, URL, key and model.
- **Streaming replies** with Markdown, code highlighting and collapsible reasoning blocks for models that "think".
- **System prompt presets** for general, engineering, security and writing tasks.
- **RAG over your files:** index text and source files and the most relevant chunks are added to the prompt. This needs an embedding model.
- **Secure storage:**
  - API keys go into the OS credential store (Windows Credential Manager, Secret Service on Linux).
  - Without a credential store, keys go into an encrypted vault protected by a master password.
  - Chat history and the document store are encrypted on disk.
  - Keys are never stored in plain text and never reach the web UI.
- **Built-in limits:** request rate and size limits, so a misbehaving UI cannot use up your API quota.
- **Mobile-friendly UI:** touch-sized controls, swipe to open the history drawer, and Android Back button support.

## Download

Releases are on the [Releases](../../releases) page:

| Platform | File |
|---|---|
| Windows | `.exe` installer (NSIS) or `.msi` |
| Linux | `.AppImage`, `.deb` or `.rpm` |
| macOS | `.dmg` (one universal app for Intel and Apple Silicon) |
| Android | `*_ecc-p384.apk` or `*_rsa-4096.apk` |
| iOS | `*_ios-unsigned.ipa` |

The two Android APKs are the same app, signed with different keys. Pick one and stay with it: Android only installs an update that is signed with the same key, so switching means uninstalling first. Check downloads against `SHA256SUMS.txt`.

The macOS and iOS builds are not signed with an Apple Developer certificate:

- **macOS:** the app is not notarized, so Gatekeeper blocks it the first time. Right-click the app and choose **Open**, or run `xattr -cr /Applications/NekkoChat.app`.
- **iOS:** the `.ipa` has to be signed before it can be installed, for example with AltStore, Sideloadly or your own certificate.

## Development

You need [Bun](https://bun.sh), [Rust](https://rustup.rs), and the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.

```sh
bun install
bun run tauri dev     # run the app with hot reload
bun run check         # type-check the frontend
bun test spec         # frontend unit tests
cd src-tauri && cargo test
```

### Linux and NVIDIA

WebKitGTK's GPU rendering freezes or goes white on the proprietary NVIDIA driver. When `/proc/driver/nvidia` exists, the app sets `WEBKIT_DISABLE_COMPOSITING_MODE=1` and `WEBKIT_DISABLE_DMABUF_RENDERER=1` automatically. On other GPUs, rendering stays hardware-accelerated. Set either variable yourself to override this.

### Building locally

```sh
bun run tauri build                  # desktop installers for the current OS
./build-android.sh ecc               # signed Android APK (or: rsa); Windows: build-android.ps1
```

The Android script needs the keystore `tauri-max-security.p12` in the project root. It reads the password from `NEKKOCHAT_KEYSTORE_PASSWORD`, or asks for it. The signed APK is written to `dist-android/`.

## Releasing

[`.github/workflows/release.yml`](.github/workflows/release.yml) builds Windows, Linux, macOS, both Android APKs and iOS. It then publishes them, with checksums, as the **latest** GitHub release.

- Push a commit to `main` whose message contains `[release]`. The release and its tag are named after the build time in Vietnam time, as `day.month.year_hour.minute.second` (for example `02.11.2026_14.30.05`).
- Pushing a tag (for example `v0.2.0`) also publishes a release, named after the tag.
- Running the workflow by hand from the Actions tab only builds, without a release.

The release is published right away, with no draft step. To pull a bad build, delete its release and tag on GitHub.

The version shown in the app and in the desktop file names comes from `version` in `src-tauri/tauri.conf.json`. Keep it in `X.Y.Z` form, because the `.msi` installer only accepts numeric versions.

The Android job needs the repository secrets `ANDROID_KEYSTORE_BASE64` (`base64 -w0 tauri-max-security.p12`) and `ANDROID_KEYSTORE_PASSWORD`. The keystore must contain both `ec-384-identity` and `rsa-4096-identity`.

## Project layout

```
src/                 SvelteKit frontend (chat UI, settings, Markdown rendering)
spec/                frontend tests
src-tauri/src/       Rust backend
  providers/         OpenAI-compatible and Anthropic streaming clients
  rag.rs             document indexing and retrieval
  secrets.rs         keychain / vault for API keys
  vault.rs           master-password vault
  data_crypto.rs     encryption of chats and documents on disk
src-tauri/gen/android/  Android project (signing is set up in app/build.gradle.kts)
```

## License

MIT
