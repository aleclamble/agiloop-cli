#!/usr/bin/env sh
set -eu

REPO="${SCHEDULER_REPO:-example.invalid/scheduler-cli}"
VERSION="${SCHEDULER_VERSION:-latest}"
INSTALL_DIR="${SCHEDULER_INSTALL_DIR:-$HOME/.local/bin}"
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS:$ARCH" in
  Linux:x86_64)
    TARGET="x86_64-unknown-linux-gnu"
    ;;
  Darwin:x86_64)
    TARGET="x86_64-apple-darwin"
    ;;
  Darwin:arm64)
    TARGET="aarch64-apple-darwin"
    ;;
  *)
    echo "unsupported platform: $OS $ARCH" >&2
    exit 1
    ;;
esac

ARCHIVE="scheduler-$TARGET.tar.gz"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

if [ "$VERSION" = "latest" ]; then
  URL="https://github.com/$REPO/releases/latest/download/$ARCHIVE"
else
  URL="https://github.com/$REPO/releases/download/$VERSION/$ARCHIVE"
fi

mkdir -p "$INSTALL_DIR"

if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$URL" -o "$TMP_DIR/$ARCHIVE"
elif command -v wget >/dev/null 2>&1; then
  wget -q "$URL" -O "$TMP_DIR/$ARCHIVE"
else
  echo "curl or wget is required" >&2
  exit 1
fi

tar -xzf "$TMP_DIR/$ARCHIVE" -C "$TMP_DIR"
install "$TMP_DIR/scheduler" "$INSTALL_DIR/scheduler"
"$INSTALL_DIR/scheduler" --version

echo "installed scheduler to $INSTALL_DIR/scheduler"
