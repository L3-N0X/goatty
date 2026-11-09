---
paths:
  - "crates/goatty-term/**"
---
# goatty-term

- This is the only crate that touches rio-vt. Keep rio-vt types out of the public API (#38 tracks the existing `TermMode` leak). rio-vt is pinned to an exact version; bump it only on purpose.
- Clients run `Terminal` with `answer_queries = false`. Only the daemon's VT answers queries.
- Back throughput claims with `cargo run --release -p goatty-term --example vtbench` numbers.
