#!/usr/bin/env bash
# Download a static FFmpeg build into ./binaries so uberconv can bundle it.
# UNTESTED helper: check the URLs/licences for your target platforms before
# shipping. The lookup order the app uses is documented in README.md.
set -euo pipefail

DEST="${1:-binaries}"
mkdir -p "$DEST"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

os="$(uname -s)"; arch="$(uname -m)"
case "$os-$arch" in
  Linux-x86_64)  url="https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz" ;;
  Linux-aarch64) url="https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-arm64-static.tar.xz" ;;
  Darwin-*)      url="https://evermeet.cx/ffmpeg/getrelease/zip" ;;   # x86_64 build; runs on Apple Silicon via Rosetta
  MINGW*|MSYS*|CYGWIN*) url="https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip" ;;
  *) echo "unsupported platform: $os-$arch" >&2; exit 1 ;;
esac

echo "downloading $url"
curl -L --fail -o "$TMP/ffmpeg-archive" "$url"

case "$url" in
  *.tar.xz) tar -xJf "$TMP/ffmpeg-archive" -C "$TMP" ;;
  *)        unzip -q "$TMP/ffmpeg-archive" -d "$TMP" ;;
esac

bin="$(find "$TMP" -type f \( -name ffmpeg -o -name ffmpeg.exe \) | head -n1)"
[ -n "$bin" ] || { echo "ffmpeg binary not found in archive" >&2; exit 1; }
cp "$bin" "$DEST/"
chmod +x "$DEST/$(basename "$bin")"
echo "installed $DEST/$(basename "$bin")"
echo "Note: these are GPL builds (libx264 etc.). Shipping them means complying with the GPL."
