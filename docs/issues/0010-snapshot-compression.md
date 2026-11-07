---
title: zstd compression for snapshots/history
status: open
labels: [protocol,performance,remote]
milestone: R1
created: 2026-11-07
---

Negotiated zstd for `Attached`/`Resync`/history payloads only; live output stays uncompressed for latency.
