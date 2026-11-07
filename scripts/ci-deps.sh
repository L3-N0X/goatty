#!/usr/bin/env bash
# System libraries needed to build goatty (GPUI: Wayland/X11, fonts, Vulkan) on Debian/Ubuntu,
# plus zsh for goatty-shell's managed-zsh tests (they skip without it).
set -euo pipefail
sudo apt-get update -q
sudo apt-get install -yq --no-install-recommends \
  pkg-config cmake clang \
  libfontconfig-dev libfreetype-dev \
  libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libx11-xcb-dev libxcb1-dev libvulkan-dev \
  libasound2-dev libzstd-dev libssl-dev \
  zsh
