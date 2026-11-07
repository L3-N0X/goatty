---
title: Interactive priority in the daemon's per-connection writer
status: open
labels: [daemon,performance,remote]
milestone: R1
created: 2026-11-07
---

With several panes on one link, a flooding pane can delay echo in another pane by up to Σ windows / bandwidth. Let small frames (echo, prompts) jump ahead of bulk frames in the connection writer queue.
