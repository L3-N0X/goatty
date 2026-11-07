# goatty

## What we're building

**A fast, batteries-included, opinionated terminal.** Every configuration is available in a GUI, including plugins for special tools.

- **Fast:** GPU-rendered (GPUI), ≥100 MB/s end to end, 120/144 Hz while flooding, 0 frames when idle, sub-millisecond input path locally.
- **Batteries included:** works out of the box. Zsh with autosuggestions, syntax highlighting, completion, Starship prompt, Atuin history, sensible keybindings (Ctrl+C copies with a selection, else interrupts), tabs/splits, and sessions that survive closing the window.
- **Opinionated:** curated defaults instead of a blank prompt. Users shouldn't have to learn dotfiles to get a good terminal. Experienced users still get a real shell and can opt out.
- **GUI for everything:** settings, profiles, themes, prompt, keybindings, and plugins are all configurable in the GUI. The underlying config (versioned JSON profile) stays inspectable and overridable.
- **Plugins for special tools:** declarative manifests (tools, shell integrations, prompt modules, schema-driven settings UI) for things like Docker, Git, Python, Node, fzf, zoxide. goatty installs them in user space and never takes over the host (no silent dotfile/PATH/login-shell changes, no sudo without asking).
- **Remote-capable from the start:** the GUI is always a client of a `goattyd`, local or on another host over SSH/Tailscale. Rendering stays local.

Full plan: `docs/goatty-project-plan.md`. Code structure and where new work goes: `docs/architecture.md`. Remote design: `docs/remote-architecture.md`. Managed zsh: `docs/managed-zsh.md`. Profiles/settings: `docs/profiles.md`. Spike results: `docs/stage0-findings.md`. Original remote idea: `docs/archive/remote-goattyd-plan.md`.

## Current status (2026-11-07)

- **Stage 0 done:** GPUI + rio-vt terminal, glyph-cache renderer, daemon-owned sessions with reattach.
- **Remote R0 done:** protocol v2 (multiplexed, ack/window flow control with skip-and-resync), `goatty-client`, `--ssh`/`--via` transports via `goattyd proxy`, auto-reconnect.
- **Restructured for Stage 1:** daemon split into a library, `goatty-config` crate, app built around a `Workspace` root view with GPUI actions/keybindings, clap CLIs.
- **Stage 3 core done:** `goatty-shell` crate: sessions run zsh with a generated ZDOTDIR (autosuggestions, syntax highlighting, completion, Starship, Atuin, fzf, OSC 133/7), user dotfiles untouched; pinned sha256-verified user-space tool installs; `goattyd shell status|install|uninstall|dismiss`, `goatty-setup` inside the shell.
- **Themes and settings CLI:** user themes (`~/.config/goatty/themes/`, importers for base16/iTerm2/Windows Terminal), theme editor, follow the desktop's light/dark, `goatty settings|theme` and `goattyd settings` commands.
- **Stage 4 core done:** client/host settings documents (`settings.json` / `host.json`, lenient versioned store, sparse saves with backups), themes, configurable keybindings, host config built as validated generations with rollback, protocol v3 (host settings + `SetPalette`), live-applying settings window (`Ctrl+,`) with preview/apply for host settings, profile export/import.
- **Next:** Stage 1 (tabs/splits, IME, box drawing, …) in parallel with remote R1, plus the rest of Stages 3/4 (GUI first-run/diagnostics, OSC 133 in the UI, settings polish). See the GitHub issues and milestones.

Roadmap: Stage 1 terminal → Stage 2 daemon/sessions (partly done) → Stage 3 managed Zsh environment → Stage 4 profile engine + settings GUI → Stage 5 plugin system → Stage 6 more shells/OSes → Stage 7 remote/web ecosystem. Remote track R1–R3 runs alongside.

## Workflow rules

- Repo: https://github.com/L3-N0X/goatty (public). Default branch `main`. Do feature work on branches and open PRs. CI must be green (`.github/workflows/ci.yml`). Releases come from `v*` tags (`release.yml` builds linux x86_64 tarballs). `scripts/ci-deps.sh` installs the GPUI system libraries on Debian/Ubuntu.

- **Every open item becomes a GitHub issue** in [L3-N0X/goatty](https://github.com/L3-N0X/goatty/issues). That covers every TODO, known bug, untested path, deferred decision or follow-up you notice, even if it's out of scope for the current task. Don't leave open items only in chat, code comments, or doc prose.
  - Create them with `gh issue create --title … --body-file … --label … --milestone …`. Reference other issues as `#N`.
  - Labels: `app client config daemon input packaging performance platform polish protocol remote rendering security shell term testing ux`, plus `bug` (create new labels only when none fit).
  - Milestones: `Stage 1`, `Stage 3`, `Stage 4`, `R1`, `R2 (later)`, `R3 (later)`, `later` (add `Stage N` milestones as stages begin).
  - Close issues via the commit/PR (`Fixes #N`) or `gh issue close N --comment "…"` saying what resolved it.
- At the end of a task, list in your reply which issues you created or closed.
- Keep this file current when the setup, goals, or conventions change.
- Measure performance claims (benchmarks below) and record the numbers in the relevant doc. Don't claim "faster" without numbers.

## Repo layout

```
crates/
  goatty-term/    terminal engine: the ONLY crate that touches rio-vt. Terminal, Frame snapshots, palette, input encoding, PTY, reattach/resync snapshots
  goatty-proto/   wire protocol v2: msg.rs (types), codec.rs (framing), paths.rs (socket path)
  goatty-client/  Connection (reconnecting, multiplexed; reader/writer/heartbeat threads), Control (blocking CLI client), transports. No UI deps
  goatty-config/  profile model: ClientSettings/HostSettings, store (lenient load, migrations, sparse save, backups), themes, keys, profile export/import, GOATTY_* overrides
  goatty-shell/   managed zsh: Launcher, rendering (zsh/*.zsh + settings.zsh/starship.toml), generations, HostService, pinned tools + installer, doctor, `shell` commands
  goatty-daemon/  lib (server, client, session, proxy) + bin goattyd: `serve`, `proxy`, `list`, `kill`, `send`, `stop`, `shell …`
  goatty-app/     GPUI client (bin `goatty`): cli, app, settings (AppSettings global), actions, workspace, session, terminal/{view,element,glyphs,keys,stats}, settings_window/, ui/
docs/             plan, architecture, findings, remote architecture, archive/
.github/          CI (fmt, clippy -D warnings, tests, bench smoke), release on v* tags, dependabot, issue/PR templates
scripts/          smoke.sh (visual), bench.sh/perf.sh (GUI throughput)
```

## Build, run, test

```sh
cargo build --release --workspace
./target/release/goatty                 # attach newest detached session (auto-starts goattyd)
./target/release/goatty --new | --attach <id> | --local
./target/release/goatty --ssh <host> [--remote-daemon PATH]
./target/release/goatty --via 'docker exec -i box goattyd proxy'
./target/release/goattyd list | stop
./target/release/goattyd shell status          # what the managed zsh loads, what's missing, startup ms
./target/release/goattyd shell install [-y] [TOOL…]   # pinned, sha256-checked, into ~/.local/share/goatty
./target/release/goattyd shell generations | diff | rollback [N]
./target/release/goatty --settings[=shell]       # also open the settings window (Ctrl+, in the app)
./target/release/goatty settings show|set|unset|validate|export|import   # no window needed
./target/release/goatty theme list|import|export|set [--for light|dark]|delete
./target/release/goattyd settings show|set|unset|validate               # host.json on this host

cargo test --workspace                  # unit + daemon e2e (reconnect, multiplex, flow control, proxy reconnect)
cargo fmt --all                          # rustfmt.toml: max_width 140; CI checks it
cargo clippy --workspace --all-targets -- -D warnings   # CI denies warnings
cargo run --release -p goatty-term --example vtbench      # VT parse throughput
cargo run --release -p goatty-daemon --example netbench   # echo latency + flood; args: proxy | slow 2m | ssh HOST
FLOOD=big.txt scripts/bench.sh goatty ./target/release/goatty --local
```

Settings live in `~/.config/goatty/settings.json` (client) and `host.json` (host); env vars override them for one run. Env: `GOATTY_SOCKET`, `GOATTY_SHELL`, `GOATTY_SHELL_MANAGED=0` (plain zsh), `GOATTY_ZSH_USER_RC=1` (also source ~/.zshrc), `GOATTY_THEME`, `GOATTY_FONT`, `GOATTY_FONT_SIZE`, `GOATTY_STATS=1`, `GOATTY_RENDER=shaped`, `RUST_LOG`.

- Tests that start a daemon or run the CLIs must use their own `GOATTY_SOCKET` and a sandboxed `HOME`. Tests that run the managed zsh use a sandboxed `$HOME`/XDG dirs (`Dirs::under`), never the real ones. Never touch the user's running daemon at `$XDG_RUNTIME_DIR/goatty/goattyd.sock` (it holds their live sessions).
- After a protocol change, an old running goattyd can't talk to the new client. Tell the user instead of killing it.
- The test build compiles gpui and is slow the first time; run long commands in the background.

## Architecture conventions

- **Daemon owns sessions; the GUI is always a client.** The daemon's VT is authoritative and is the only one that answers terminal queries. The client mirrors it by feeding the same bytes into its own `Terminal` (`answer_queries = false`).
- **Wire = raw VT bytes + VT-byte snapshots**, never rio-vt types. `Attached` snapshots start with RIS; `Resync` snapshots must not reset (the client keeps its scrollback).
- **Flow control is in the protocol:** clients `Ack` consumed bytes. Past `window`, the daemon skips and later sends `Resync`. Local window 8 MiB (lossless), remote 256 KiB.
- **Protocol compatibility:** `Hello` stays variant 0 with `version` as its first field in both enums. Bump `PROTOCOL_VERSION` on any wire change.
- **Never block the UI thread on I/O:** writes go through the client's writer thread; the UI is woken through the coalescing `Waker` (one notify per frame).
- **Threads, not async** (until a QUIC transport needs tokio). Encode a broadcast frame once and share it via `Arc<[u8]>`.
- Don't use `std::io::copy` between pipes and Unix sockets: on Linux it uses `splice()`, which stalled the proxy. Use plain read/write loops, and write binary data to stdout through the raw fd.
- **Settings go through `goatty-config`.** Don't read `GOATTY_*` env vars elsewhere (only `GOATTY_SOCKET`, in `goatty_proto::socket_path`). The app reads client settings from the `AppSettings` global and edits them only via `AppSettings::update`; host settings go through the daemon (`HostLink`/`HostService`), never by writing another host's file. Loading never fails: bad values fall back with an `Issue`. Saves write only non-defaults.
- **Host config changes are generations:** build, `zsh -n`, then atomic `current` switch; rollback restores the generation's `host.json` too. Don't write into `current/` directly.
- **Shortcuts are GPUI actions** (`goatty-app/src/actions.rs`, `Terminal` key context), never `match` arms in `on_key`. Bound actions run before the raw key handler; unbound keys go to the shell via `terminal/keys.rs`.
- **Panes don't touch the window.** `TerminalView` emits `TerminalEvent`s; `Workspace` owns titles, closing, and (later) tabs/splits.
- Workspace lints live in the root `Cargo.toml` (`[workspace.lints]`); every crate has `[lints] workspace = true`. Every `unsafe` block needs a `// SAFETY:` comment.
- Dependencies: rio-vt pinned exactly; gpui from Zed git at a pinned rev (bump deliberately: change `rev`, then `cargo update -p gpui`). Edition 2024, `opt-level = 1` in dev with deps at 3.
- **Managed zsh** (`goatty-shell/zsh/`): user files are sourced at top level (never inside a function), with the user's own `ZDOTDIR`; syntax highlighting loads last; `typeset name=(…)` without `builtin`. Change startup files only with `tests/zsh_startup.rs` green. Tool pins need a sha256 from the real download, checked against upstream's checksum file.
- Safety first: no silent edits to dotfiles, global PATH or the login shell (goatty only writes under its own XDG dirs); OSC 52 clipboard reads are denied; the daemon socket is 0600 in a 0700 dir and never exposed on the network.
