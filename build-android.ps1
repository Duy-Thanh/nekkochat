param (
    [Parameter(Mandatory=$true, HelpMessage="Specify 'rsa' or 'ecc'")]
    [ValidateSet("rsa", "ecc")]
    [string]$Type
)

# --- CONFIGURATION ---
$KeystoreFile = ".\tauri-max-security.p12"
$OutputDir    = ".\dist-android"

# The keystore password is never stored in this file. Set NEKKOCHAT_KEYSTORE_PASSWORD in your
# shell, or you will be prompted for it.
$KeystorePass = $env:NEKKOCHAT_KEYSTORE_PASSWORD
if (-not $KeystorePass) {
    $secure = Read-Host "Keystore password" -AsSecureString
    $bstr = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secure)
    try { $KeystorePass = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($bstr) }
    finally { [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($bstr) }
}
# -------------------------------------------------

# Ensure the output directory exists
if (-not (Test-Path $OutputDir)) { 
    New-Item -ItemType Directory -Path $OutputDir | Out-Null 
}

# Resolve the absolute path of the keystore file
if (-not (Test-Path $KeystoreFile)) {
    Write-Error "Error: Keystore file not found at $KeystoreFile"
    exit 1
}
$KeystoreAbsPath = [System.IO.Path]::GetFullPath($KeystoreFile)

# Set the correct cryptographic identity alias
if ($Type -eq "rsa") {
    Write-Host "`n[RSA] Building Android APK with Heavyweight RSA 4096..." -ForegroundColor Cyan
    $env:TAURI_ANDROID_KEY_ALIAS = "rsa-4096-identity"
} elseif ($Type -eq "ecc") {
    Write-Host "`n[ECC] Building Android APK with Ultra-Efficient ECC P-384..." -ForegroundColor Green
    $env:TAURI_ANDROID_KEY_ALIAS = "ec-384-identity"
}

# Set required Tauri environment variables
$env:TAURI_ANDROID_KEYSTORE_PATH = $KeystoreAbsPath
$env:TAURI_ANDROID_KEYSTORE_PASSWORD = $KeystorePass
$env:TAURI_ANDROID_KEY_PASSWORD = $KeystorePass

# Run the Tauri build, then always remove the password from this session's environment
try {
    npm run tauri android build -- --apk
} finally {
    Remove-Item Env:TAURI_ANDROID_KEYSTORE_PASSWORD, Env:TAURI_ANDROID_KEY_PASSWORD -ErrorAction SilentlyContinue
    $KeystorePass = $null
    # The Gradle daemon keeps the environment it was started with (including the password) in
    # memory, so stop it now that the build is done.
    Push-Location "src-tauri\gen\android"
    try { & .\gradlew.bat --stop 2>&1 | Out-Null } catch { } finally { Pop-Location }
}

# Where Tauri/Gradle put the APK. A build that was not signed is named "-unsigned".
$ApkDir      = "src-tauri\gen\android\app\build\outputs\apk\universal\release"
$SignedApk   = Join-Path $ApkDir "app-universal-release.apk"
$UnsignedApk = Join-Path $ApkDir "app-universal-release-unsigned.apk"

if (-not (Test-Path $SignedApk)) {
    if (Test-Path $UnsignedApk) {
        Write-Error ("The build finished but the APK is UNSIGNED ($UnsignedApk), so it cannot be installed or published. " +
                     "Signing is configured in src-tauri\gen\android\app\build.gradle.kts and reads the keystore from " +
                     "TAURI_ANDROID_KEYSTORE_PATH, so check that file still contains the signingConfigs block.")
    } else {
        Write-Error "Error: Build finished but could not find a release APK in $ApkDir"
    }
    exit 1
}

# Verify the signature with the Android SDK's apksigner when it is available
$ApkSigner = $null
$SdkRoots = @($env:ANDROID_HOME, $env:ANDROID_SDK_ROOT, "$env:LOCALAPPDATA\Android\Sdk") |
    Where-Object { $_ -and (Test-Path (Join-Path $_ "build-tools")) }
foreach ($root in $SdkRoots) {
    $ApkSigner = Get-ChildItem (Join-Path $root "build-tools") -Filter "apksigner.bat" -Recurse -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1
    if ($ApkSigner) { break }
}
if ($ApkSigner) {
    & $ApkSigner.FullName verify --print-certs $SignedApk
    if ($LASTEXITCODE -ne 0) {
        Write-Error "apksigner could not verify the signature of $SignedApk"
        exit 1
    }
} else {
    Write-Warning "apksigner was not found (set ANDROID_HOME or ANDROID_SDK_ROOT), so the signature was not verified."
}

# Safely move and rename the APK to prevent overwriting
Move-Item -Path $SignedApk -Destination "$OutputDir\app-release-$Type.apk" -Force
Write-Host "`n============================================================" -ForegroundColor Gray
Write-Host "Success! Signed APK saved to:" -ForegroundColor Green
Write-Host "Path: $OutputDir\app-release-$Type.apk" -ForegroundColor Yellow
Write-Host "============================================================" -ForegroundColor Gray
