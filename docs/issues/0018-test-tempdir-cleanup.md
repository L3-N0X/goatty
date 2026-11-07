---
title: Integration tests and netbench leave temp dirs in /tmp
status: open
labels: [testing]
milestone: R1
created: 2026-11-07
---

`crates/goatty-daemon/tests/reconnect.rs` and `examples/netbench.rs` create `goatty-test-*` / `goatty-netbench-*` under `std::env::temp_dir()` and never remove them. Use a drop guard or the `tempfile` crate.
