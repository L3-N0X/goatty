---
title: Size policy for multiple clients on one session
status: open
labels: [daemon,protocol]
milestone: R1
created: 2026-11-07
---

Today the last resize wins. Introduce one controlling client per session that sets the PTY size; others are observers (letterboxed) or take over explicitly.
