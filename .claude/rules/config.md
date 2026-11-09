---
paths:
  - "crates/goatty-config/**"
---
# goatty-config

Model and file format: `docs/profiles.md`.

- Loading never fails. A bad value falls back to its default and records an `Issue`. Saves write only non-default values, and keep backups.
- `GOATTY_*` env overrides are read here and nowhere else.
- When the format changes, bump `Document::VERSION` and add a `(from, fn(&mut Value))` entry to `migrations()`. Old files must keep loading.
