---
title: Smoke-test the GUI after the protocol v2 / goatty-client switch
status: open
labels: [app,testing]
milestone: R1
created: 2026-11-07
---

`goatty-app` was moved to `goatty-client::Connection` (protocol v2) and compiles, but the window was never opened after the change.

**To do**
- Run `goatty`, `goatty --new`, `goatty --attach <id>`, `goatty --local`, `scripts/smoke.sh`.
- Check the title decoration (`— user@host`, `(reconnecting…)`) with `--via 'env GOATTY_SOCKET=… goattyd proxy'`.
- Re-run `scripts/bench.sh` (GUI throughput) and compare with Stage 0 (~100 MB/s).
