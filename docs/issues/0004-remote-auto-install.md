---
title: Auto-install a matching goattyd on remote hosts + version-range negotiation
status: open
labels: [remote,protocol]
milestone: R1
created: 2026-11-07
---

Today both ends need the exact same protocol version. If `Hello` fails or the version differs, offer to upload a matching static goattyd (x86_64/aarch64 musl) to `~/.local/share/goatty/bin/goattyd-<ver>` over the same ssh and use it (like VS Code Server). Replace exact match with min/max version negotiation. Keep the `Hello` compatibility rule (variant 0, `version` first).
