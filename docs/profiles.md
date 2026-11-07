# Profiles and settings (Stage 4)

The profile is the source of truth for everything goatty manages. The settings window edits it; files stay inspectable JSON. Code: `crates/goatty-config` (model, store), `crates/goatty-shell` (`generation`, `service`), `crates/goatty-app` (`settings`, `settings_window`, `ui`).

## Two documents: client and host (#14)

| Document | File (`$XDG_CONFIG_HOME/goatty/`) | Lives on | Contains |
|---|---|---|---|
| `ClientSettings` | `settings.json` | the machine running the window | font, theme, cursor, padding, renderer, keybindings, workspace (on launch), debug |
| `HostSettings` | `host.json` | each machine running a goattyd | shell program, managed zsh and its features, user dotfiles, prompt, history |

The split follows where a setting takes effect. The window draws text, so fonts and colors are the client's. Shells run on the daemon's host, so their configuration belongs to that host. A remote host keeps its own `host.json`, and the settings window edits it through the protocol. Connecting to a different host shows that host's shell settings.

## The store

`goatty_config::store` reads and writes both documents:

- **Lenient load.** A missing file means defaults. Invalid JSON means defaults plus an issue, and the file is left as it is. An invalid value (`"size": "big"`, an unknown enum variant, a negative `u32`) reverts *that value* to its default and becomes an `Issue { path: "font.size", message }`. Every other value is kept. This uses `serde_path_to_error` in a loop: deserialize, find the failing path, put the default back there, retry.
- **Unknown keys** (for example, written by a newer goatty) are reported and kept. Saving puts them back. Maps with user-chosen keys (`keybindings`) are exempt (`Document::OPEN_MAPS`).
- **Versions and migrations.** Each document has a `"version"`. Older versions run through `Document::migrations()`, a list of `(from_version, fn(&mut Value))`. A newer version is read as well as possible and flagged.
- **Validation.** `Document::validate` clamps ranges (font size 6–48, line height 1–2, padding 0–48, history 100–10M), reports unknown theme and action names, and drops empty values.
- **Sparse save.** Only values that differ from the defaults are written, so improved defaults reach everyone who never changed that setting. The previous file goes to `backups/<name>-<unix time>.json` (newest 20 kept), and the new one replaces it atomically (temp file + rename). Unchanged content isn't rewritten.
- **Environment overrides.** `GOATTY_*` variables (table in `goatty-config/src/lib.rs`) apply on top for one run and are never saved. The settings window edits the file value and lists the overrides that are active.

## Applying host settings: generations

Host settings compile into shell configuration (plan §6). `goatty_shell::generation`:

1. Render the startup files from the settings (`zsh::render`: `settings.zsh`, `starship.toml`, static `.zshenv/.zprofile/.zshrc/integration.zsh`).
2. If the active generation has identical files *and* the same settings snapshot, do nothing.
3. Otherwise write `generations/.build-<pid>/` with `zsh/`, `host.json` (the snapshot, without env overrides) and `info.json`.
4. Validate with `zsh -n` on every startup file. On failure, discard the build: nothing changes.
5. Rename the build to `generations/<n>` and switch `current` to it by renaming a new symlink over the old one (atomic).
6. Keep the newest 10.

Sessions get `ZDOTDIR=~/.local/share/goatty/current/zsh`. Paths written into the config go through `current`, so the files don't depend on the generation number. Builders lock `generations/.lock` (`File::lock`), so goattyd, `goatty --local` and the CLI can't race.

**Rollback** switches `current` to an older generation *and* restores its `host.json` snapshot as the settings file (backing up the current one). Without restoring the settings, the next session would rebuild from the newer settings and undo the rollback.

**Apply order** (`HostService::set`): parse leniently → compute the diff against the active generation → build + validate + activate → save `host.json`. A configuration that doesn't validate is rejected before the file is touched.

## Protocol (v3)

| Request | Reply | Notes |
|---|---|---|
| `GetHostSettings` | `HostSettings { json, issues, env_overrides, generation }` | `json` is the stored document |
| `SetHostSettings { json, apply }` | `HostSettingsApplied { issues, diff, applied, generation }` | `apply: false` = validate + preview only |
| `ListGenerations` | `Generations { list }` | |
| `Rollback { generation }` | `HostSettings` | `None` = the previous one |
| `SetPalette { session, colors }` | – | 21 `0xRRGGBB`: ANSI 16, fg, bg, cursor, selection fg/bg |

Settings travel as JSON documents rather than typed structs, so a schema change doesn't need a protocol bump. The daemon's parser handles versions and unknown keys.

`SetPalette` fixes #36: the daemon's terminal is authoritative and answers OSC 4/10/11 color queries, so it needs the colors the user actually sees. Clients send their theme after attaching and on every theme change, and resend it after a reconnect. With several clients on one session, the last one wins.

## The app

- `settings::AppSettings` (GPUI global) holds the loaded file, the effective value (file + env, with the font family resolved against the installed fonts), the installed font list and problems. `AppSettings::update` edits, validates, saves and replaces the global. Every view observes the global, so font, size, line height, padding, renderer, cursor shape/blink and theme apply live. The file is polled every 500 ms, so edits in an editor apply live too.
- Keybindings: `goatty_config::keys` holds the action table and defaults. `actions::apply_keybindings` rebuilds every binding with `KeyBinding::load`, so an invalid keystroke is reported instead of panicking.
- Theme: `ThemeSettings { name, bold_is_bright, overrides }` resolves to `ThemeColors`. The terminal palette is swappable at runtime (`Terminal::set_palette`), and goatty's own UI colors derive from it (`ui::style::Ui`).
- Ctrl+Shift+= / - zoom on top of the configured size until Ctrl+0 resets it. Changing the size in settings keeps the zoom offset.

### The settings window

`Ctrl+,` (or `goatty --settings[=page]`) opens it. Pages:

| Page | Edits | Applies |
|---|---|---|
| Appearance | theme gallery, bold-is-bright, font size/line height/renderer, font family (filterable list), padding, cursor shape/blink | live, saved immediately |
| Shell | program, managed zsh, user dotfiles, autosuggestions/highlighting/fzf/integration | staged → preview → apply |
| Prompt | Starship, rich/minimal style, two-line, blank line, duration threshold, prefer own starship.toml | staged |
| History | size, duplicates, leading space, sharing, Atuin, Atuin on Up | staged |
| Shortcuts | per action: key chips (× removes), + records a new keystroke, reset | live |
| Workspace | reattach newest session vs. start new on launch | next launch |
| Profile | file locations, problems, env overrides, export/import, reset, generations with rollback | – |

Host edits collect in a draft. A bar at the bottom offers **Discard**, **Preview** (the host computes the diff of the generated files, shown colored) and **Apply** (a new generation, validated first). Calls to the daemon run off the UI thread. With `--local` there is no daemon, and the window uses `goatty_shell::HostService` in-process: same code path, same files.

**Export** writes `{ "goatty_profile": 1, "client": {...}, "host": {...} }` with non-default values only, minus machine-specific ones (`host.shell.program`, `client.debug`). **Import** applies the client part right away and stages the host part as a draft to preview and apply.

## Verified

- `goatty-config`: 20 unit tests (lenient parsing per field, unknown keys kept across saves, versions, backups and pruning, themes, keybinding edits, profile round trip without private values).
- `goatty-shell`: generations (idempotence, numbering, settings-only changes, rollback, pruning, `zsh -n` rejection), `HostService` (preview/apply/rollback), launcher through `current`, and the real-zsh tests from Stage 3.
- `goatty-daemon/tests/settings.rs`: the protocol against a real goattyd with a sandboxed HOME (get, preview, apply, a bad value, generations, rollback, and `SetPalette` changing the actual OSC 11 answer read back from the PTY).
- Manually, release build, sandboxed HOME: the window renders every page. Editing `settings.json` from outside switched the terminal and the window to Catppuccin Latte, 17 px, bar cursor within the poll interval, and reported the bad value and the unknown key on the Profile page. `goattyd shell diff/status/generations/rollback` behaved as described.
- Not verified yet: clicking through the controls (needs interactive testing, #49).

## Not done yet

Theme editor and user themes (#50), following the system light/dark preference (#51), a headless settings CLI (#52), problems and env overrides shown on their row (#53), shortcut conflict warnings (#54), per-generation change summaries (#55), reloading running shells after apply (#56), proper monospace detection (#57), keyboard navigation in the settings window (#58), and the GUI first-run/diagnostics screen (#40).
