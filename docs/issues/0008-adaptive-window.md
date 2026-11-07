---
title: Adaptive flow-control window
status: open
labels: [daemon,client,performance,remote]
milestone: R1
created: 2026-11-07
---

Measure delivery rate from acks and RTT from pings, then set `window ≈ max(64 KiB, rate × 50 ms)`. A fast LAN stays lossless, a slow link skips early. Today it's a fixed `StreamOpts::REMOTE` (256 KiB).
