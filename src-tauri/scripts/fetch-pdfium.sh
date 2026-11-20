#!/bin/bash
# Downloads the pinned pdfium build into src-tauri/vendor/pdfium and checks its SHA-256.
# Source: https://github.com/bblanchon/pdfium-binaries
set -euo pipefail

TAG="chromium/8138"
ASSET="pdfium-mac-arm64.tgz"
SHA256="21a1567b397b571f4f77f21ca91eac7b892774219c3c715a5127a20950ccb18b"

dest="$(cd "$(dirname "$0")/.." && pwd)/vendor/pdfium"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

gh release download "$TAG" -R bblanchon/pdfium-binaries -p "$ASSET" -D "$tmp"
echo "$SHA256  $tmp/$ASSET" | shasum -a 256 -c -
mkdir -p "$dest" "$tmp/x"
tar -xzf "$tmp/$ASSET" -C "$tmp/x"
cp "$tmp/x/lib/libpdfium.dylib" "$tmp/x/LICENSE" "$dest/"
rm -rf "$dest/licenses"
cp -R "$tmp/x/licenses" "$dest/licenses"
echo "pdfium installed in $dest"
