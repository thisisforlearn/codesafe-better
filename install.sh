#!/usr/bin/env bash
# install.sh — Install codeguard/codesafe on Linux, macOS, and Termux
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/thisisforlearn/codesafe-better/main/install.sh | bash
#
# Installs to:
#   - ~/bin/ on Termux / Linux
#   - /usr/local/bin/ if run as root on Linux (optional)
#   - ~/bin/ or /usr/local/bin/ on macOS
#
set -euo pipefail

REPO="thisisforlearn/codesafe-better"
BIN_NAME="codeguard"
ALIAS_NAME="codesafe"

# Detect platform
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$ARCH" in
  x86_64|amd64) ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
  *) echo "Unsupported architecture: $ARCH" ; exit 1 ;;
esac

case "$OS" in
  linux)   PLATFORM="linux" ;;
  darwin)  PLATFORM="macos" ;;
  *) echo "Unsupported OS: $OS" ; exit 1 ;;
esac

# Choose install directory
if [ "${EUID:-$(id -u)}" -eq 0 ] && [ -d /usr/local/bin ]; then
  INSTALL_DIR="/usr/local/bin"
else
  INSTALL_DIR="$HOME/bin"
  mkdir -p "$INSTALL_DIR"
fi

echo "==> Installing codeguard to $INSTALL_DIR"

# Try downloading a prebuilt release from GitHub
RELEASES_API="https://api.github.com/repos/${REPO}/releases/latest"
ASSET_NAME="codeguard-${PLATFORM}-${ARCH}"

DOWNLOAD_URL=""
if command -v curl >/dev/null 2>&1; then
  echo "==> Checking GitHub releases for ${ASSET_NAME}..."
  DOWNLOAD_URL=$(curl -fsSL "$RELEASES_API" 2>/dev/null \
    | grep -o "https://.*${ASSET_NAME}[^\"]*" | head -1 || true)
fi

if [ -n "$DOWNLOAD_URL" ]; then
  echo "==> Downloading ${DOWNLOAD_URL}..."
  curl -fsSL -o "$INSTALL_DIR/$BIN_NAME" "$DOWNLOAD_URL"
else
  echo "==> No prebuilt release found. Building from source (requires cargo)..."
  if ! command -v cargo >/dev/null 2>&1; then
    echo "Error: cargo not found. Install Rust first: https://rustup.rs/"
    exit 1
  fi
  tmpdir=$(mktemp -d)
  git clone "https://github.com/${REPO}.git" "$tmpdir"
  cd "$tmpdir"
  cargo build --release
  cp "target/release/$BIN_NAME" "$INSTALL_DIR/$BIN_NAME"
  cd -
  rm -rf "$tmpdir"
fi

chmod +x "$INSTALL_DIR/$BIN_NAME"

# Create alias codesafe -> codeguard
if [ ! -e "$INSTALL_DIR/$ALIAS_NAME" ]; then
  ln -s "$BIN_NAME" "$INSTALL_DIR/$ALIAS_NAME" 2>/dev/null || cp "$INSTALL_DIR/$BIN_NAME" "$INSTALL_DIR/$ALIAS_NAME"
fi

# Ensure INSTALL_DIR is on PATH
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "" && echo "==> Add $INSTALL_DIR to your PATH:" && echo "   export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac

echo "==> Done! Run:  codeguard --help   or   codesafe --help"
echo "==> Example:    codeguard --output json test ~/test-py-code"
