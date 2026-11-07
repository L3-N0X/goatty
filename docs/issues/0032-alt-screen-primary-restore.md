---
title: Reattach can't restore the primary screen hidden behind the alt screen
status: open
labels: [term,daemon]
milestone: later
created: 2026-11-07
---

rio-vt keeps the inactive grid private, so after reattaching during e.g. vim, leaving vim shows an empty primary screen. Needs an upstream accessor or a VT core switch (alacritty_terminal fallback).
