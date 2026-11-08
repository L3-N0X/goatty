# goatty architecture

How the code is laid out, and where each upcoming stage's work goes. For the product plan see [`goatty-project-plan.md`](goatty-project-plan.md); for the remote design see [`remote-architecture.md`](remote-architecture.md).

## Crate graph

```
                 goatty-config      (profile model: client/host JSON documents, store, themes, keys, profiles)
                   ▲        ▲   ▲
                   │        │   │   goatty-plugin  (plugin format, permissions, signed registry index; bin goatty-plugin)
                   │        │   │      ▲
                   │        │   goatty-shell    (managed zsh: generated ZDOTDIR, tool installs, plugins on a host, diagnostics)
                   │        │      ▲      ▲
goatty-term ◄──────┼────────┼──────┼──────┼─┐   (the only crate that touches rio-vt)
   ▲               │        │      │      │ │
   │   goatty-proto│        │      │      │ │   (wire protocol: messages, framing, socket path)
   │      ▲        │        │      │      │ │
   │   goatty-client        │      │      │ │   (Connection, Control, transports; no UI)
   │      ▲     ▲           │      │      │ │
   │      │     └──── goatty-daemon ──────┘─┘   (lib + bin `goattyd`)
   │      │                        │
   └── goatty-app ─────────────────┘            (GPUI client, bin `goatty`)
```

Rules:

- **rio-vt stays inside `goatty-term`.** Other crates use `Terminal`, `Frame`, `TermSize`, `TermMode` and the input encoders. `Terminal::with_term` is crate-private on purpose.
- **`goatty-proto` has no I/O beyond framing.** Transports live in `goatty-client`.
- **`goatty-client` has no UI dependencies.** The app, the daemon's CLI, tests and benchmarks all use it.
- **Which shell a session runs is `goatty-shell`'s decision** (`Launcher::launch`). goattyd and `goatty --local` both use it; `goatty-term` only spawns what it's given. `goatty-shell` has no rio-vt or UI dependency.
- **Client vs host settings** (#14): `ClientSettings` (`settings.json`) is read by the app; `HostSettings` (`host.json`) by goattyd/`HostService` on its own host. The GUI edits host settings through the protocol (`GetHostSettings`, `SetHostSettings`, …), never by writing a remote file. See [`profiles.md`](profiles.md).
- **`goatty-plugin` knows the format, not the host.** Manifests, settings schema, templates, permissions, zsh rendering, the signed index and the JSON API live there with no knowledge of `Dirs`; `goatty-shell::plugins` decides where things go. The authoring/registry CLI (`goatty-plugin`) depends on nothing else in the workspace, so registry CI builds it quickly. See [`plugins.md`](plugins.md).
- **Settings come from `goatty-config`.** Don't read `GOATTY_*` variables elsewhere (the exception is `GOATTY_SOCKET`, which is a location, not a setting, and lives in `goatty_proto::socket_path`).

## Daemon (`crates/goatty-daemon`)

| Module       | Owns                                                                     |
|--------------|--------------------------------------------------------------------------|
| `server`     | socket bind (0600 in a 0700 dir), accept loop, socket watchdog, the session registry (`Daemon`), shutdown |
| `client`     | one connection: decode requests, its subscriptions, the writer thread    |
| `session`    | PTY + authoritative `Terminal` + subscribers with flow control, the output pump |
| `proxy`      | `goattyd proxy`: stdio ⇄ socket relay for remote transports              |
| `main.rs`    | clap CLI; control commands go through `goatty_client::Control`           |

Threads per session: `s{id}-reader` (PTY → terminal → subscribers) and `s{id}-replies` (query answers → PTY). Per connection: a reader (the accept thread's spawn) and `c{conn}-writer`.

## Shell environment (`crates/goatty-shell`)

| Module    | Owns                                                                    |
|-----------|-------------------------------------------------------------------------|
| `launch`  | `Launcher`: resolve the shell program, write the managed ZDOTDIR, build the `Launch` (program, args, env) |
| `zsh`     | render `settings.zsh` + the static startup files in `zsh/`, write them atomically |
| `tools`   | pinned tool catalog (versions, URLs, sha256), finding system/managed copies |
| `install` | download (curl/wget), verify, unpack, activate symlinks, uninstall      |
| `doctor`  | start a real managed shell and parse `goatty-status`                    |
| `generation` | build → validate (`zsh -n`) → activate generations; list, rollback, diff |
| `service` | `HostService`: host settings get/set(preview, apply)/generations/rollback, for the daemon API and `goatty --local` |
| `cmd`     | `goattyd shell status|install|uninstall|dismiss|generations|diff|rollback` |
| `plugins` | plugins on this host: `registry` (trusted sources, fetch → verify → snapshot), `lock` (installed files + approvals), `Plugins` (find, plan, install, uninstall, catalog), `active` (what a generation renders), `cmd` (`goattyd plugin …`) |
| `dirs`    | XDG locations under `goatty/`                                           |

Design and startup sequence: [`managed-zsh.md`](managed-zsh.md). The zsh behaviour is tested against real zsh in `tests/zsh_startup.rs` (sandboxed `$HOME`, PTY for OSC 133/7).

## App (`crates/goatty-app`)

```
main.rs        parse CLI (subcommands run without a window), open the Session, app::run
commands.rs    `goatty settings …` / `goatty theme …` (headless)
cli.rs         clap definition
app.rs         GPUI bootstrap: settings, host link, main window, OpenSettings handler
settings.rs    AppSettings global: client settings file ⇄ live values, file polling, keybindings
actions.rs     actions, name → action map, loading user keybindings (Terminal key context)
command_palette/
  mod.rs       the Ctrl+Shift+P overlay: ranking (fuzzy + keywords + recently used), list, keys
  registry.rs  Command (id, category, title, keywords, action or handler), CommandRegistry global, built-ins
  fuzzy.rs     fzf-style matcher: word starts, consecutive runs, gaps; any word order; highlight positions
settings_window/
  mod.rs       the settings window: pages, host draft → preview → apply, export/import
  host.rs      HostLink: the daemon connection or the local HostService
  pages.rs     rendering of every page
settings_window/plugins.rs  the Plugins page: catalog, install review (plan), schema-driven setting controls
toast/
  mod.rs       Toast (level, title, body, buttons, key, lifetime) and the API: show/show_in/update/dismiss/dismiss_all, routed to a window's layer
  stack.rs     one window's toasts without UI: replace by key, eviction, pausable timers (unit-tested with explicit clocks)
  layer.rs     ToastLayer view: bottom-right cards, spring enter/exit, pause on hover / inactive window, one timer task
ui/
  style.rs     UI colors derived from the terminal theme
  icons.rs     Icon enum + Assets (SVGs from assets/icons/, embedded, served to svg())
  controls.rs  toggle, segmented, stepper, button, rows
  text_input.rs single-line text field (IME-aware; adapted from GPUI's example)
  color_picker.rs color picker widget: `Entity<ColorPicker>` emitting `Changed`/`Committed`; `Pick` (float state), `Wheel` geometry, painting, drag handling
workspace.rs   root view per window: owns panes, window title, closes on exit, shows the command palette and the toast layer
session.rs     a pane's data: local PTY or goattyd session → mirror Terminal; coalescing waker
terminal/
  view.rs      TerminalView: input → terminal, actions, emits TerminalEvent
  element.rs   custom grid Element (prepaint/paint)
  glyphs.rs    per-cell glyph cache
  keys.rs      GPUI keystroke → terminal Key (pure, unit-tested)
  stats.rs     GOATTY_STATS frame statistics
```

View tree: `Workspace` → `TerminalView` (one today). A `TerminalView` never touches the window directly. It emits `TerminalEvent::{TitleChanged, Exited}` and the workspace decides what that means (window title, close the pane or the window).

Feedback: anything that finishes without a visible result (a palette command, a background request, later plugins) reports through `crate::toast`: `toast::show(cx, Toast::success(…))`, or `Toast::loading` first and `toast::update` with the result. Give repeated feedback a `key` so it replaces itself. Toasts never take focus.

Input: GPUI dispatches a bound action before the view's raw `on_key_down`. Shortcuts are actions in `actions.rs`, and anything unbound goes to the shell through `terminal::keys` and `goatty_term::input`. Don't add shortcuts as `match` arms in `on_key`.

## Where upcoming work goes

| Work                                   | Where                                                                 |
|----------------------------------------|-----------------------------------------------------------------------|
| Tabs and splits (#30)                  | `workspace.rs`: tab strip + split tree of `Entity<TerminalView>`; new panes reuse the window's `Connection` |
| IME / compose (#21)                    | `terminal/view.rs`: implement `EntityInputHandler`, register it in the element's paint |
| Box drawing (#24)                      | `terminal/element.rs` (+ a new `terminal/boxdraw.rs`)                 |
| Kitty keyboard (#22)                   | `goatty-term/src/input.rs`, fed from `terminal/keys.rs`               |
| URL detection (#27)                    | `goatty-term` (detect on `Frame` rows) + view (hover/click)           |
| Shell integration, terminal side (#28) | `goatty-term` listener (OSC 133/7 → `UiEvent`) + daemon `SessionInfo`; the zsh side is in `goatty-shell/zsh/integration.zsh` |
| Managed zsh features, tool versions    | `goatty-shell/zsh/zshrc.zsh` (load order matters, see `managed-zsh.md`), `goatty-shell/src/tools.rs` |
| GUI first-run / shell diagnostics      | daemon API for `goatty_shell::cmd`-style status/install (new wire messages), then a GPUI view |
| Bash/Fish adapters (Stage 6)           | `goatty-shell`: a sibling of `zsh.rs` + its startup files; `Launcher::launch` picks by program |
| New client setting                     | field in `goatty-config/src/client.rs` (+ `validate`), control in `settings_window/pages.rs`, apply in `TerminalView::apply_settings` |
| New host setting                       | field in `goatty-config/src/host.rs`, render it in `goatty-shell/src/zsh.rs` (`settings.zsh`/`starship.toml`), control in `pages.rs` |
| A color setting / editing a color      | embed `Entity<ColorPicker>` (`ui/color_picker.rs`): `open(color)` to start, `sync(color)` to follow outside changes, react to `ColorPickerEvent::Changed` (live; `AppSettings::preview`) and `Committed` (save). Parsing and formats: `goatty-config/src/colorspace.rs` |
| New theme import format                | `goatty-config/src/theme_import.rs` (detect + parse to `ThemeColors`) |
| Settings migration                     | bump `Document::VERSION`, add `(from, fn(&mut Value))` to `migrations()` |
| systemd user service (#7)              | `goatty-daemon` (`server::serve` already takes the socket path)       |
| A plugin feature (new manifest field)  | `goatty-plugin/src/manifest.rs` (+ validation, permissions if it can run code), rendering in `goatty-plugin/src/zsh.rs`, document it in `docs/plugins.md` and the template |
| A plugin operation (GUI/CLI)           | `goatty_plugin::api::Request`/`Reply` (no protocol bump), `HostService::plugin_request`, `settings_window/plugins.rs`, `plugins/cmd.rs` |
| New wire messages                      | `goatty-proto/src/msg.rs` (bump `PROTOCOL_VERSION`), handle in `goatty-daemon/src/client.rs` and `goatty-client/src/connection.rs` |
