---
title: Lazy scrollback fetch from the daemon
status: open
labels: [protocol,daemon,client]
milestone: R1
created: 2026-11-07
---

Add `FetchHistory{session, before_line, count}` so the client fills its scrollback on demand. This covers both the trimmed attach snapshot (2000 lines remote) and lines skipped by a `Resync`. It needs stable absolute line numbering in snapshots.
