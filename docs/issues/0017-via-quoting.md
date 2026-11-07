---
title: `--via` splits on whitespace
status: open
labels: [client,ux]
milestone: R1
created: 2026-11-07
---

`Endpoint::command_line` uses `split_whitespace`, so arguments containing spaces can't be passed. Use shell-words parsing or run the command through `sh -c`.
