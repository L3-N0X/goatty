# goatty

## What we're building

**A fast, batteries-included, opinionated terminal.** Every configuration is available in a GUI, including plugins for special tools.

- **Fast:** GPU-rendered (GPUI), ≥100 MB/s end to end, 120/144 Hz while flooding, 0 frames when idle, sub-millisecond input path locally.
- **Batteries included:** works out of the box. Zsh with autosuggestions, syntax highlighting, completion, Starship prompt, Atuin history, sensible keybindings (Ctrl+C copies with a selection, else interrupts), tabs/splits, and sessions that survive closing the window.
- **Opinionated:** curated defaults instead of a blank prompt. Users shouldn't have to learn dotfiles to get a good terminal. Experienced users still get a real shell and can opt out.
- **GUI for everything:** settings, profiles, themes, prompt, keybindings, and plugins are all configurable in the GUI. The underlying config (versioned JSON profile) stays inspectable and overridable.
- **Plugins for special tools:** declarative manifests (tools, shell integrations, prompt modules, schema-driven settings UI) for things like Docker, Git, Python, Node, fzf, zoxide. goatty installs them in user space and never takes over the host (no silent dotfile/PATH/login-shell changes, no sudo without asking).
- **Remote-capable from the start:** the GUI is always a client of a `goattyd`, local or on another host over SSH/Tailscale. Rendering stays local.

Full plan: `docs/goatty-project-plan.md`. Remote design: `docs/remote-architecture.md`. Spike results: `docs/stage0-findings.md`. Original remote idea: `remote-goattyd-plan.md`.

## Current status (2026-11-07)

- **Stage 0 done:** GPUI + rio-vt terminal, glyph-cache renderer, daemon-owned sessions with reattach.
- **Remote R0 done:** protocol v2 (multiplexed, ack/window flow control with skip-and-resync), `goatty-client`, `--ssh`/`--via` transports via `goattyd proxy`, auto-reconnect.
- **Next:** Stage 1 (usable native terminal: tabs/splits, IME, box drawing, …) in parallel with remote R1. See `docs/issues/`.

Roadmap: Stage 1 terminal → Stage 2 daemon/sessions (partly done) → Stage 3 managed Zsh environment → Stage 4 profile engine + settings GUI → Stage 5 plugin system → Stage 6 more shells/OSes → Stage 7 remote/web ecosystem. Remote track R1–R3 runs alongside.

## Workflow rules

- **Every open item becomes an issue.** Write an issue for every TODO, known bug, untested path, deferred decision, or follow-up you notice, even if it's out of scope for the current task. Don't leave open items only in chat, code comments, or doc prose.
  - There is no GitHub remote yet, so issues live in `docs/issues/NNNN-slug.md` (front matter: `title`, `status: open|done|wontfix`, `labels`, `milestone`, `created`), indexed in `docs/issues/README.md`. Use the next free number and add a row to the index.
  - When you fix one, set `status: done`, add a line on what resolved it, and update the index.
  - Once a remote exists, migrate them with `gh issue create` and track issues on GitHub from then on.
- At the end of a task, list in your reply which issues you created or closed.
- Keep this file current when the setup, goals, or conventions change.
- Measure performance claims (benchmarks below) and record the numbers in the relevant doc. Don't claim "faster" without numbers.

## Repo layout

```
crates/
  goatty-term/    terminal engine: the ONLY crate that touches rio-vt. Frame snapshots, palette, input encoding, PTY, reattach/resync snapshots
  goatty-proto/   wire protocol v2 (postcard, length-prefixed, transport-independent)
  goatty-client/  Connection to a goattyd: transports (local socket, ssh, any stdio command), reader/writer/heartbeat threads, reconnect. No UI deps
  goatty-daemon/  goattyd: owns PTYs + authoritative VT per session; `serve`, `proxy`, `list`, `kill`, `send`, `stop`
  goatty-app/     GPUI desktop client (bin `goatty`): custom grid Element, glyph cache, input, selection
docs/             plan, findings, remote architecture, issues/
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

cargo test --workspace                  # unit + daemon e2e (reconnect, multiplex, flow control, proxy reconnect)
cargo clippy --workspace --all-targets
cargo run --release -p goatty-term --example vtbench      # VT parse throughput
cargo run --release -p goatty-daemon --example netbench   # echo latency + flood; args: proxy | slow 2m | ssh HOST
FLOOD=big.txt scripts/bench.sh goatty ./target/release/goatty --local
```

Env: `GOATTY_SOCKET`, `GOATTY_SHELL`, `GOATTY_FONT`, `GOATTY_FONT_SIZE`, `GOATTY_STATS=1`, `GOATTY_RENDER=shaped`, `RUST_LOG`.

- Tests that start a daemon must use their own `GOATTY_SOCKET`. Never touch the user's running daemon at `$XDG_RUNTIME_DIR/goatty/goattyd.sock` (it holds their live sessions).
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
- Dependencies: rio-vt pinned exactly; gpui from Zed git at a pinned rev (bump deliberately: change `rev`, then `cargo update -p gpui`). Edition 2024, `opt-level = 1` in dev with deps at 3.
- Safety first: no silent edits to dotfiles, global PATH or the login shell; OSC 52 clipboard reads are denied; the daemon socket is 0600 in a 0700 dir and never exposed on the network.
