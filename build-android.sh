#!/bin/bash

# Exit immediately if a command exits with a non-zero status
set -euo pipefail

TYPE="${1:-}"
if [ "$TYPE" != "rsa" ] && [ "$TYPE" != "ecc" ]; then
    echo "❌ Error: Please specify 'rsa' or 'ecc'" >&2
    echo "Usage: ./build-android.sh [rsa|ecc]" >&2
    exit 1
fi

# --- CONFIGURATION ---
KEYSTORE_FILE="./tauri-max-security.p12"
OUTPUT_DIR="./dist-android"

# The keystore password is never stored in this file. Set NEKKOCHAT_KEYSTORE_PASSWORD in your
# shell, or you will be prompted for it.
KEYSTORE_PASS="${NEKKOCHAT_KEYSTORE_PASSWORD:-}"
if [ -z "$KEYSTORE_PASS" ]; then
    read -r -s -p "Keystore password: " KEYSTORE_PASS
    echo
fi
# -------------------------------------------------

# Ensure the output directory exists
mkdir -p "$OUTPUT_DIR"

# Resolve the absolute path of the keystore file
if [ ! -f "$KEYSTORE_FILE" ]; then
    echo "❌ Error: Keystore file not found at $KEYSTORE_FILE" >&2
    exit 1
fi
KEYSTORE_ABS_PATH=$(realpath "$KEYSTORE_FILE")

# Set the correct cryptographic identity alias
if [ "$TYPE" == "rsa" ]; then
    echo "🔒 Building Android APK with Heavyweight RSA 4096..."
    export TAURI_ANDROID_KEY_ALIAS="rsa-4096-identity"
else
    echo "⚡ Building Android APK with Ultra-Efficient ECC P-384..."
    export TAURI_ANDROID_KEY_ALIAS="ec-384-identity"
fi

# Export required Tauri environment variables
export TAURI_ANDROID_KEYSTORE_PATH="$KEYSTORE_ABS_PATH"
export TAURI_ANDROID_KEYSTORE_PASSWORD="$KEYSTORE_PASS"
export TAURI_ANDROID_KEY_PASSWORD="$KEYSTORE_PASS"

# After the build (or if it fails), remove the password from the environment. The Gradle daemon
# keeps the environment it was started with (including the password) in memory, so stop it too.
cleanup() {
    unset TAURI_ANDROID_KEYSTORE_PASSWORD TAURI_ANDROID_KEY_PASSWORD KEYSTORE_PASS NEKKOCHAT_KEYSTORE_PASSWORD
    (cd src-tauri/gen/android && ./gradlew --stop >/dev/null 2>&1) || true
}
trap cleanup EXIT

# Run the Tauri build
npm run tauri android build -- --apk
cleanup
trap - EXIT

# Where Tauri/Gradle put the APK. A build that was not signed is named "-unsigned".
APK_DIR="src-tauri/gen/android/app/build/outputs/apk/universal/release"
SIGNED_APK="$APK_DIR/app-universal-release.apk"
UNSIGNED_APK="$APK_DIR/app-universal-release-unsigned.apk"

if [ ! -f "$SIGNED_APK" ]; then
    if [ -f "$UNSIGNED_APK" ]; then
        echo "❌ Error: The build finished but the APK is UNSIGNED ($UNSIGNED_APK), so it cannot be installed or published." \
             "Signing is configured in src-tauri/gen/android/app/build.gradle.kts and reads the keystore from" \
             "TAURI_ANDROID_KEYSTORE_PATH, so check that file still contains the signingConfigs block." >&2
    else
        echo "❌ Error: Build finished but could not find a release APK in $APK_DIR" >&2
    fi
    exit 1
fi

# Verify the signature with the Android SDK's apksigner when it is available
APKSIGNER=""
for root in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" "$HOME/Android/Sdk"; do
    [ -n "$root" ] && [ -d "$root/build-tools" ] || continue
    APKSIGNER=$(find "$root/build-tools" -name apksigner -type f 2>/dev/null | sort -V | tail -n 1)
    [ -n "$APKSIGNER" ] && break
done
if [ -z "$APKSIGNER" ] && command -v apksigner >/dev/null 2>&1; then
    APKSIGNER=$(command -v apksigner)
fi
if [ -n "$APKSIGNER" ]; then
    if ! "$APKSIGNER" verify --print-certs "$SIGNED_APK"; then
        echo "❌ Error: apksigner could not verify the signature of $SIGNED_APK" >&2
        exit 1
    fi
else
    echo "⚠️  Warning: apksigner was not found (set ANDROID_HOME or ANDROID_SDK_ROOT), so the signature was not verified." >&2
fi

# Safely move and rename the APK to prevent overwriting
mv -f "$SIGNED_APK" "$OUTPUT_DIR/app-release-$TYPE.apk"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ Success! Signed APK saved to:"
echo "📦 $OUTPUT_DIR/app-release-$TYPE.apk"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
