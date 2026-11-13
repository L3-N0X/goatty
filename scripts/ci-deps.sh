#!/usr/bin/env bash
# System libraries needed to build goatty (GPUI: Wayland/X11, fonts, Vulkan) on Debian/Ubuntu,
# plus zsh for goatty-shell's managed-zsh tests (they skip without it).
set -euo pipefail

pkgs=(
  pkg-config cmake clang
  libfontconfig-dev libfreetype-dev
  libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev
  libx11-xcb-dev libxcb1-dev libvulkan-dev
  libasound2-dev libzstd-dev libssl-dev
  zsh
)

for attempt in 1 2 3; do
  if sudo apt-get -o Acquire::Retries=5 update -q \
    && sudo apt-get -o Acquire::Retries=5 install -yq --no-install-recommends "${pkgs[@]}"; then
    exit 0
  fi
  if [ "$attempt" -lt 3 ]; then
    echo "apt-get failed (attempt $attempt/3); retrying in 5s..." >&2
    sleep 5
  fi
done
exit 1
