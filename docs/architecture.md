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
- **Settings come from `goatty-config`.** Don't read `GOATTY_*` variables elsewhere (the exceptions: `GOATTY_SOCKET`, a location, in `goatty_proto::socket_path`; `GOATTY_ASKPASS`, goatty's marker to itself as ssh's askpass, in `goatty_client::askpass`).
- **Remote hosts** (`goatty-client`): `transport` (endpoints; ssh with captured stderr and askpass), `remote` (the connect script and its `goatty-remote:` probe line, `RemoteProblem`, install/stop over ssh, bundled binaries), `askpass` (decode ssh's question), `service` (systemd user units), `ssh_config` (the hosts in `~/.ssh/config`, for the host list). The app turns `RemoteProblem`s into questions in `connect.rs`. See [`remote-architecture.md`](remote-architecture.md) §5a/§5b.

## Daemon (`crates/goatty-daemon`)

| Module       | Owns                                                                     |
|--------------|--------------------------------------------------------------------------|
| `server`     | socket bind (0600 in a 0700 dir) or the socket from systemd (`LISTEN_FDS`), accept loop, socket watchdog, the session registry (`Daemon`: shells and `Run` commands), shutdown |
| `client`     | one connection: decode requests, its subscriptions, the writer thread    |
| `session`    | PTY + authoritative `Terminal` + subscribers with flow control, the output pump |
| `proxy`      | `goattyd proxy`: stdio ⇄ socket relay for remote transports              |
| `main.rs`    | clap CLI; control commands go through `goatty_client::Control`; `stop --force` (any protocol version, via `SO_PEERCRED`), `service install|uninstall|status` |

Threads per session: `s{id}-reader` (PTY → terminal → subscribers) and `s{id}-replies` (query answers → PTY). Per connection: a reader (the accept thread's spawn) and `c{conn}-writer`.

## Shell environment (`crates/goatty-shell`)

| Module    | Owns                                                                    |
|-----------|-------------------------------------------------------------------------|
| `launch`  | `Launcher`: resolve the shell program, write the managed ZDOTDIR, build the `Launch` (program, args, env) |
| `zsh`     | render `settings.zsh`, the plugin slot files (`plugins-*.zsh`) + the static startup files in `zsh/`, write them atomically |
| `builtins`| the plugins that come with goatty (`plugins/<id>/`, compiled in): autosuggestions, syntax highlighting, Starship, Atuin, fzf |
| `packages`| where a plugin's package is: system copy, goatty's managed copy, or missing |
| `install` | download (curl/wget), verify, unpack, activate `bin/` and `plugins/` symlinks, uninstall packages |
| `doctor`  | start a real managed shell and parse `goatty-status`                    |
| `generation` | build → validate (`zsh -n`) → activate generations; list, rollback, diff |
| `service` | `HostService`: host settings get/set(preview, apply)/generations/rollback, for the daemon API and `goatty --local` |
| `cmd`     | `goattyd shell status|install|uninstall|dismiss|generations|diff|rollback` |
| `plugins` | plugins on this host: `registry` (trusted sources, fetch → verify → snapshot), `lock` (installed files + approvals), `Plugins` (find, plan, install, uninstall, catalog; built-in ones included), `active` + `load_order` (what a generation renders, in slot order), `cmd` (`goattyd plugin …`) |
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
  mod.rs       the Ctrl+Shift+P overlay: ranking (fuzzy + keywords + recently used), list, keys; hosts once something is typed; `Scope::Hosts` (Ctrl+Shift+H): only the hosts, plus a typed user@host
  registry.rs  Command (id, category, title, keywords, action or handler), CommandRegistry global, built-ins
  fuzzy.rs     fzf-style matcher: word starts, consecutive runs, gaps; any word order; highlight positions
settings_window/
  mod.rs       the settings window: pages, host draft → preview → apply, export/import
  host.rs      HostLink: the daemon connection or the local HostService
  pages.rs     rendering of every page
settings_window/plugins.rs  the Plugins page: catalog, install review (plan), schema-driven setting controls
settings_window/hosts.rs    the Hosts page: add/remove hosts, plain SSH per host, `~/.ssh/config` discovery
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
workspace/
  mod.rs       root view per window: tabs of split panes, spawning/closing panes, window title, closes with the last pane; command palette, paste confirmations, toast layer
  layout.rs    a tab's split tree (pure, unit-tested): n-ary splits with weights, split/remove/normalize, layout → leaf rects + dividers, neighbor, resize
  tab_bar.rs   the tab strip in both styles (`tabs.style`: modern / compact): switch, close, rename, drag to reorder
  icons.rs     process name → Nerd Font glyph + theme color; finds a font that really has the glyphs (terminal font, then Symbols Nerd Font)
  saved.rs     the window's layout as goattyd keeps it (JSON: tabs, split trees with session ids, names, focus, zoom); `restore` claims and reattaches it
close_confirm.rs  "Close this pane?" dialog when a program other than the shell still runs (`panes.confirm_close`, "Don't ask again")
connect.rs     connect window for remote endpoints: connects off the UI thread, turns RemoteProblem into questions (install / update and restart / plain SSH), opens the terminal window
hosts.rs       hosts to connect to: `ssh.hosts` + `~/.ssh/config` (`HostList` global, read in the background), palette commands, `connect` → connect window
askpass.rs     goatty as ssh's SSH_ASKPASS: one window per prompt (passphrase, host key, confirm), answer on stdout
paste_confirm.rs  "Paste this?" dialog: risks, preview with control/invisible chars marked, paste as is / cleaned / as one line
session.rs     a pane's data: local PTY or goattyd session → mirror Terminal; coalescing waker; foreground program; `Spawner` for new panes
terminal/
  view.rs      TerminalView: input → terminal, actions, emits TerminalEvent
  element.rs   custom grid Element (prepaint/paint)
  glyphs.rs    per-cell glyph cache
  keys.rs      GPUI keystroke → terminal Key (pure, unit-tested)
  links.rs     what a detected link means here (URL, other scheme → ask, local file if it exists, remote path → copy), file openers
  stats.rs     GOATTY_STATS frame statistics
```

View tree: `Workspace` → tab bar + the active tab's panes (`TerminalView`s, absolutely positioned from `layout::Node::layout`). A `TerminalView` never touches the window directly. It emits `TerminalEvent::{TitleChanged, Exited, ConfirmPaste}` and the workspace decides what that means (tab and window title, close the pane, its tab or the window, show the paste dialog and hand the chosen text back via `TerminalView::paste_text`).

Tabs and panes (#30): every pane is its own session from the window's `Spawner` (another `Create` on the same `Connection`, or another local PTY), started in the focused pane's OSC 7 directory and created off the UI thread. Closing a pane kills its session; closing the window only detaches. Closing asks first (`close_confirm.rs`) when a pane's foreground program isn't a shell and `panes.confirm_close` is on. Pane and tab shortcuts are actions handled on the workspace's root div (they bubble up from the focused `TerminalView`); Alt+digit / Alt+arrow `cx.propagate()` when there's no such tab or pane, so the key still reaches the shell. Dividers are GPUI drags (`DividerDrag` + `on_drag_move` on the pane area). Background tabs aren't rendered, so their output costs no frames.

Restoring windows (#118): the workspace sends its layout with `Connection::save_layout` (protocol v7 `SaveLayout`, fire-and-forget, only when the JSON changed, re-sent after a reconnect) whenever tabs, splits, sizes, names, focus or zoom change. The JSON (`workspace/saved.rs`, `version` 1) is opaque to goattyd, which only keeps the session list and which connection holds the layout (`goatty-daemon/src/layouts.rs`). A connection ending frees its layout. `goatty` in reattach mode sends `ClaimLayout`: the daemon hands out the newest free layout that still shows a running session, to one window only (a second window gets the next free layout, else the newest detached session as before, else a new shell). Sessions that ended drop out of the tree (`Node::map_leaves`). `--new`, `--attach` and `--local` start a fresh layout.

Foreground program: `goatty_term::process::ForegroundWatch` checks the PTY's foreground process group (`tcgetpgrp` → `/proc/<pid>/comm|cmdline`) shortly after output or input, never while idle. goattyd sends changes as `ServerMsg::Process` (v6; also right after `Attached`, and in `SessionInfo.process`); `--local` watches its own PTY. Tabs use it for the icon and as the title fallback (app title → program unless it's a shell → directory name).

Links (#27): `goatty_term::Terminal::link_at(col, row)` finds the OSC 8 hyperlink, URL or path candidate under a cell, on the whole wrapped line, only when asked (mouse move / modifier change / new output while hovering), so idle and flood frames pay nothing. `terminal/links.rs` resolves it (relative paths against the OSC 7 directory, which rio-vt only accepts from this host) and the view underlines it, shows the target at the bottom of the pane, and opens it on Ctrl+click (`links.open_on`).

Paste (#29): every paste goes through `goatty_term::paste::check` (runs commands without bracketed paste, control or invisible characters, `curl … | sh`, > 256 KiB, multiline); `paste.confirm` decides which of those ask first.

Feedback: anything that finishes without a visible result (a palette command, a background request, later plugins) reports through `crate::toast`: `toast::show(cx, Toast::success(…))`, or `Toast::loading` first and `toast::update` with the result. Give repeated feedback a `key` so it replaces itself. Toasts never take focus.

Input: GPUI dispatches a bound action before the view's raw `on_key_down`. Shortcuts are actions in `actions.rs`, and anything unbound goes to the shell through `terminal::keys` and `goatty_term::input`. Don't add shortcuts as `match` arms in `on_key`.

## Where upcoming work goes

| Work                                   | Where                                                                 |
|----------------------------------------|-----------------------------------------------------------------------|
| Tab/pane features (layout restore, titlebar tabs, …) | `workspace/` (`mod.rs` for behaviour, `layout.rs` for tree operations, `tab_bar.rs` for looks); a new program icon: `workspace/icons.rs` |
| IME / compose (#21)                    | `terminal/view.rs`: implement `EntityInputHandler`, register it in the element's paint |
| Box drawing (#24)                      | `terminal/element.rs` (+ a new `terminal/boxdraw.rs`)                 |
| Kitty keyboard (#22)                   | `goatty-term/src/input.rs`, fed from `terminal/keys.rs`               |
| Shell integration, terminal side (#28) | `goatty-term` listener (OSC 133/7 → `UiEvent`) + daemon `SessionInfo`; the zsh side is in `goatty-shell/zsh/integration.zsh` |
| Managed zsh features, tool versions    | `goatty-shell/zsh/zshrc.zsh` (load order matters, see `managed-zsh.md`); the tools are built-in plugins: `goatty-shell/plugins/<id>/plugin.toml` (+ `builtins.rs` to list a new one) |
| GUI first-run / shell diagnostics      | daemon API for `goatty_shell::cmd`-style status/install (new wire messages), then a GPUI view |
| Bash/Fish adapters (Stage 6)           | `goatty-shell`: a sibling of `zsh.rs` + its startup files; `Launcher::launch` picks by program |
| New client setting                     | field in `goatty-config/src/client.rs` (+ `validate`), control in `settings_window/pages.rs`, apply in `TerminalView::apply_settings` |
| New host setting                       | field in `goatty-config/src/host.rs`, render it in `goatty-shell/src/zsh.rs` (`settings.zsh`/`starship.toml`), control in `pages.rs` |
| A color setting / editing a color      | embed `Entity<ColorPicker>` (`ui/color_picker.rs`): `open(color)` to start, `sync(color)` to follow outside changes, react to `ColorPickerEvent::Changed` (live; `AppSettings::preview`) and `Committed` (save). Parsing and formats: `goatty-config/src/colorspace.rs` |
| New theme import format                | `goatty-config/src/theme_import.rs` (detect + parse to `ThemeColors`) |
| Settings migration                     | bump `Document::VERSION`, add `(from, fn(&mut Value))` to `migrations()` |
| FDSTORE / shells across restarts (#131) | `goatty-daemon/src/server.rs` (restore), `goatty-client/src/service.rs` (unit options) |
| Host list / switcher (#4)              | the host list is `hosts.rs` (palette) + `settings_window/hosts.rs`; per-host data in `goatty-config` `SshSettings`; `~/.ssh/config` parsing in `goatty-client/src/ssh_config.rs`. OS/RTT and a session picker: `Connection::host()`/`rtt()`/`list_sessions` |
| A plugin feature (new manifest field)  | `goatty-plugin/src/manifest.rs` (+ validation, permissions if it can run code), rendering in `goatty-plugin/src/zsh.rs`, document it in `docs/plugins.md` and the template |
| A plugin operation (GUI/CLI)           | `goatty_plugin::api::Request`/`Reply` (no protocol bump), `HostService::plugin_request`, `settings_window/plugins.rs`, `plugins/cmd.rs` |
| New wire messages                      | `goatty-proto/src/msg.rs`: a new variant at the *end* (append-only), bump `PROTOCOL_VERSION`, mark it `vN:`; handle in `goatty-daemon/src/client.rs` and `goatty-client/src/connection.rs`, which must not send it to a daemon that agreed on an older version (`Connection::protocol_version`) |
