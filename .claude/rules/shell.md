---
paths:
  - "crates/goatty-shell/**"
---
# goatty-shell (managed zsh)

Design and load order: `docs/managed-zsh.md`.

- In the startup files (`zsh/`):
  - Source user files at top level, never inside a function, with the user's own `ZDOTDIR`.
  - Syntax highlighting loads last (`zsh.load = "last"`).
  - Write `typeset name=(…)` without `builtin`.
  - Only change these files with `tests/zsh_startup.rs` passing.
- Host config changes are generations: build, check with `zsh -n`, then switch `current` atomically. A rollback also restores the generation's `host.json`. Never write into `current/` directly.
- The built-in tools are plugins (`plugins/<id>/plugin.toml`, listed in `builtins.rs`). A new pin must be a release at least 2 weeks old, with a sha256 taken from the real download and checked against upstream's checksum file. Keep their package ids: installs from before #98 live in `tools/<id>/`.
- Tests that run zsh use `Dirs::under` a temp dir, never the real `$HOME` or XDG dirs.
