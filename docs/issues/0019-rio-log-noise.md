---
title: Silence rio-vt warnings for unhandled OSC (e.g. systemd OSC 3008)
status: open
labels: [term,polish]
milestone: Stage 1
created: 2026-11-07
---

rio-vt logs `[unhandled osc_dispatch]` at WARN for every systemd OSC 3008 context sequence, which floods daemon logs and test output. Default `rio_vt=error` in the env_logger filters (daemon, app, examples).
