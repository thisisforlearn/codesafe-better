# install.ps1 — Install codeguard/codesafe on Windows (PowerShell)
#
# Usage (PowerShell):
#   Set-ExecutionPolicy Bypass -Scope Process -Force
#   irm https://raw.githubusercontent.com/thisisforlearn/codesafe-better/main/install.ps1 | iex
#
# Installs to: ~/bin/codeguard.exe and ~/bin/codesafe.exe (alias)
#
$ErrorActionPreference = "Stop"

$REPO = "thisisforlearn/codesafe-better"
$BIN_NAME = "codeguard.exe"
$ALIAS_NAME = "codesafe.exe"

# Detect architecture (Windows PowerShell 5 compatible)
$ARCH = if ([Environment]::Is64BitProcess) { "x86_64" } else { "i686" }

# Choose install directory
$INSTALL_DIR = Join-Path $env:USERPROFILE "bin"
if (-not (Test-Path $INSTALL_DIR)) {
  New-Item -ItemType Directory -Path $INSTALL_DIR -Force | Out-Null
}

Write-Host "==> Installing codeguard to $INSTALL_DIR" -ForegroundColor Cyan

# Try downloading a prebuilt release from GitHub
$RELEASES_API = "https://api.github.com/repos/$REPO/releases/latest"
$ASSET_NAME = "codeguard-windows-${ARCH}.exe"
$DOWNLOAD_URL = $null

try {
  Write-Host "==> Checking GitHub releases for ${ASSET_NAME}..." -ForegroundColor Cyan
  $release = Invoke-RestMethod -Uri $RELEASES_API -TimeoutSec 15
  $asset = $release.assets | Where-Object { $_.name -like "*${ASSET_NAME}*" } | Select-Object -First 1
  if ($asset) {
    $DOWNLOAD_URL = $asset.browser_download_url
  }
} catch {
  Write-Host "==> Could not query GitHub API: $($_.Exception.Message)" -ForegroundColor Yellow
}

if ($DOWNLOAD_URL) {
  Write-Host "==> Downloading $DOWNLOAD_URL ..." -ForegroundColor Cyan
  Invoke-WebRequest -Uri $DOWNLOAD_URL -OutFile (Join-Path $INSTALL_DIR $BIN_NAME)
} else {
  Write-Host "==> No prebuilt release found. Building from source (requires cargo)..." -ForegroundColor Yellow
  if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo not found. Install Rust first: https://rustup.rs/"
  }
  $tmpdir = [System.IO.Path]::GetTempPath() + "codesafe-install-" + [System.Guid]::NewGuid().ToString("N")
  New-Item -ItemType Directory -Path $tmpdir -Force | Out-Null
  git clone "https://github.com/$REPO.git" $tmpdir
  Push-Location $tmpdir
  cargo build --release
  Copy-Item -Path "target/release/$BIN_NAME" -Destination (Join-Path $INSTALL_DIR $BIN_NAME) -Force
  Pop-Location
  Remove-Item -Path $tmpdir -Recurse -Force
}

# Create alias codesafe.exe -> codeguard.exe
$aliasPath = Join-Path $INSTALL_DIR $ALIAS_NAME
if (-not (Test-Path $aliasPath)) {
  try {
    New-Item -ItemType SymbolicLink -Path $aliasPath -Target (Join-Path $INSTALL_DIR $BIN_NAME) -Force | Out-Null
  } catch {
    Copy-Item -Path (Join-Path $INSTALL_DIR $BIN_NAME) -Destination $aliasPath -Force
  }
}

# Ensure INSTALL_DIR is on PATH
$currentPath = [Environment]::GetEnvironmentVariable("PATH", [EnvironmentVariableTarget]::User)
if (-not ($currentPath -split ";" | Where-Object { $_ -eq $INSTALL_DIR })) {
  [Environment]::SetEnvironmentVariable("PATH", "$currentPath;$INSTALL_DIR", [EnvironmentVariableTarget]::User)
  Write-Host "==> Added $INSTALL_DIR to your PATH. Restart your terminal." -ForegroundColor Yellow
}

Write-Host "==> Done! Run:  codeguard --help   or   codesafe --help" -ForegroundColor Green
Write-Host "==> Example:    codeguard --output json test C:\path\to\project" -ForegroundColor Green
