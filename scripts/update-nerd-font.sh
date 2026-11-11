#!/usr/bin/env bash
# Fetch the bundled JetBrainsMono Nerd Font into crates/goatty-app/assets/fonts.
# Pinned release and checksum: bump both on purpose, then rebuild and check
# that the icon test (`cargo test -p goatty-app icons`) still passes.
set -euo pipefail

VERSION=v3.5.1
SHA256=04d5e8f903693f9dd13e16f867e994834e681eb3c72c0d337a770dcda09010cf
FACES=(Regular Bold Italic BoldItalic)

root=$(cd "$(dirname "$0")/.." && pwd)
dest="$root/crates/goatty-app/assets/fonts"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/JetBrainsMono.tar.xz" \
  "https://github.com/ryanoasis/nerd-fonts/releases/download/$VERSION/JetBrainsMono.tar.xz"
# sha256sum (coreutils) or shasum (macOS, perl).
if command -v sha256sum >/dev/null; then sha256=(sha256sum); else sha256=(shasum -a 256); fi
echo "$SHA256  $tmp/JetBrainsMono.tar.xz" | "${sha256[@]}" -c -
# Only what's bundled: the whole archive unpacks to ~250 MB.
files=(OFL.txt)
for face in "${FACES[@]}"; do files+=("JetBrainsMonoNerdFont-$face.ttf"); done
tar -xJf "$tmp/JetBrainsMono.tar.xz" -C "$dest" "${files[@]}"
echo "updated $dest to Nerd Fonts $VERSION"
