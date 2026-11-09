---
paths:
  - "crates/goatty-plugin/**"
  - "crates/goatty-shell/src/plugins/**"
  - "crates/goatty-shell/plugins/**"
---
# Plugins

Format and trust model: `docs/plugins.md`.

- Plugins are data, and they are checked twice:
  - Permissions are derived from the manifest, never declared by the plugin itself.
  - Installs carry the plan digest the user approved.
  - Everything that gets sourced is re-checked against the lock.
- Quote all generated shell code with `goatty_plugin::zsh::quote`. The only code a plugin runs is its reviewed `zsh.init`.
- A new pin must be a release at least 2 weeks old, with its sha256 matched against upstream's digest.
- To add a manifest field: add it to `manifest.rs` (with validation, and permissions if it can run code), render it in `zsh.rs`, and document it in `docs/plugins.md` and in the template repo.
- Related repos:
  - `../goatty-registry` (https://github.com/goatty-dev/goatty-registry) is the official registry. Its index is signed offline with the key in `~/.local/share/goatty-registry-keys/`. Never commit that key or put it in CI.
  - `../goatty-plugin-template` (https://github.com/goatty-dev/goatty-plugin-template) is the template for plugin authors.
