# Remote goattyd: architecture and plan

**Date:** 2026-11-07, updated 2026-11-09 · **Status:** R0 implemented (protocol v2, SSH/stdio transports, flow control, reconnect). R1 in progress: version negotiation, goattyd auto-install, plain SSH fallback, SSH prompts in the GUI and the systemd user service are done (§5a, §5b).

**Goal:** the GUI connects to *any* goattyd: on this machine, on a home server over Tailscale, on a cloud box over SSH, or in a container. Rendering stays local on the GPU. Sessions live on the host and survive the client. Local performance must not regress, and remote must stay usable on poor links.

This turns the ideas in [`archive/remote-goattyd-plan.md`](archive/remote-goattyd-plan.md) into concrete decisions and makes them consistent with what Stage 0 measured ([`stage0-findings.md`](stage0-findings.md)).

---

## 1. Decisions

| # | Decision | Why |
|---|---|---|
| D1 | **The GUI is always a client.** Local mode uses the same protocol and code path as remote; `--local` (window-owned PTY) stays only as a debugging fallback. | One code path to keep correct and fast. "Local" is just the transport with the biggest window. |
| D2 | **The daemon's VT is authoritative; the client keeps a mirror VT fed from the byte stream.** No grid diffs on the wire. | See §2. Bytes are smaller than diffs for typical output. Scrollback, selection and search stay local with no round trips. A future web client can feed the same bytes straight into xterm.js. |
| D3 | **Flow control lives in the protocol (acks + window), and a client that falls behind is skipped forward with a `Resync` snapshot.** | Over SSH, the kernel, ssh and TCP buffers hide megabytes, so socket backpressure is meaningless. Skip-and-resync gives mosh's key property, bounded latency on slow links, without mosh's state-sync protocol. |
| D4 | **One connection per host, multiplexing every session** (Stage 0 used one connection per pane). | Each SSH connection costs a handshake (100 ms to 1 s) and a process. Per-pane flow control now comes from per-subscriber windows (D3), not from per-socket backpressure. |
| D5 | **Remote transport = a stdio command running `goattyd proxy` on the host.** `ssh host` is the default. | goatty contains no SSH code, yet gets `~/.ssh/config`, agent, keys, ProxyJump, ControlMaster, known_hosts and Tailscale SSH for free. The same mechanism works for `docker exec`, `kubectl exec` and `distrobox`. The daemon never listens on a network port. |
| D6 | **Wire types are goatty-owned** (`goatty-proto`). rio-vt types never cross the wire; snapshots are plain VT byte streams. | Swapping the VT core (rio → alacritty_terminal) doesn't break remote compatibility. |
| D8 | **Two ways to reach a host: goattyd (default) or plain SSH (fallback). No third mode.** | See §5a. goattyd gives persistence, reconnect, the managed shell, host settings and plugins; goatty installs it with one confirmation. Plain SSH is `ssh host` in a session of the *local* goattyd, for hosts goattyd can't or shouldn't run on. A raw "remote PTY over an ssh channel" mode would fork every feature into two code paths (against D1) for no gain over plain SSH. |
| D9 | **Peers negotiate a protocol version from a range; the wire grows append-only.** | §4. A GUI update no longer strands the sessions in a running daemon, as long as the ranges overlap. |
| D7 | **Threads, not async**, still. Per connection: reader + writer thread. Per session: PTY reader + reply thread. | Simple and fast at the scale of "tens of sessions". Revisit only for a QUIC transport (R2), where quinn requires tokio. |

## 2. Why bytes plus a mirror VT, not structured screen updates

The pasted plan proposed that the server send "structured terminal updates". We already have the important half of that, since the daemon owns the canonical state and reconnects from it. For the live stream, raw bytes are the better encoding:

| | Raw VT bytes + client mirror (chosen) | Grid diffs (tmux control mode / zellij style) |
|---|---|---|
| Typing, prompts, `ls` | A few bytes per change | A row diff per change: larger |
| `cat huge.log` on a fast link | Lossless, ~100 MB/s locally | Diffs per frame: fewer bytes, loses scrollback |
| `cat huge.log` on a slow link | Skip + resync (D3): bounded | Naturally bounded |
| Scrollback, search, selection | Local, instant | Needs a fetch protocol |
| Client CPU | Parses bytes twice in total (≥175 MB/s each, negligible) | Applies diffs |
| Web client later | xterm.js eats the bytes as-is | Needs a custom renderer |
| Protocol surface | Tiny | Large: cell attrs, images, hyperlinks, wide chars… |

The only case where diffs win is the slow-link flood, and D3 covers it. Grid diffs would still matter for a very thin client that can't run a VT (a watch face?). That isn't a goal.

## 3. Layers

```text
 goatty (GPUI)                                         host
┌──────────────────────────┐                ┌──────────────────────────────┐
│ TerminalView / Element   │                │ goattyd                      │
│   mirror Terminal (rio)  │                │   Session ─ PTY ─ zsh        │
│ goatty-client::Connection│                │     authoritative Terminal   │
│   reader ─ dispatch/ack  │                │     subscribers (window/ack) │
│   writer ─ never blocks UI                │   per-connection writer      │
│   heartbeat (remote)     │                │                              │
└───────────┬──────────────┘                └──────────────▲───────────────┘
            │ framed postcard (goatty-proto v2)            │ Unix socket 0600
            ▼                                              │
   Endpoint::Local ──── Unix socket ───────────────────────┘
   Endpoint::Command ── ssh host 'goattyd proxy' ── stdio ── goattyd proxy ─┘
                        docker exec -i box goattyd proxy …
```

Crates: `goatty-proto` (wire types, framing), `goatty-client` (transports, `Connection`, `PaneSink`; no UI dependencies, reusable by a future CLI or TUI), `goatty-daemon`, `goatty-app`, `goatty-term`.

## 4. Protocol v2 (implemented)

Framing: `u32` LE length + postcard. Byte payloads use `serde_bytes`, so a 64 KiB chunk carries a ~5-byte header.

- **Handshake and versions (v6):** `Hello{version, client}` → `Hello{version, daemon_pid, host: HostInfo}`. Each build speaks `MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION` (now 5–6). The client announces its newest version; the daemon answers with `min(client, its newest)`, or with its own newest and hangs up when that's below its minimum, so the client can tell "daemon too old" from "daemon too new" (`VersionMismatch`). Afterwards neither side sends a message newer than the agreed version (e.g. `Connection::run` needs v6). This only works because the enums are **append-only**: new messages are new variants at the end, existing ones never change, each addition bumps `PROTOCOL_VERSION` and is marked `vN:` in `msg.rs`; anything else raises `MIN_PROTOCOL_VERSION`. Daemons up to v5 required an exact match and reply `Error("protocol mismatch: daemon vN, …")`; the client parses that once and reconnects announcing `vN`, so a new GUI keeps working with a running v5 daemon.
- **`Run` (v6):** like `Create`, but the session runs a given `argv` instead of the user's shell (plain SSH uses it).
- **Requests** carry a client-chosen `ReqId`: `ListSessions`, `Create`, `Attach`, `Kill`, `SendInput`, `Shutdown` → `Sessions | Attached | Ok | Error{req}`.
- **Streams are keyed by `SessionId`.** The daemon encodes each PTY chunk **once** and shares the same `Arc<[u8]>` frame with every subscriber on every connection.
- **Stream:** `Attached{snapshot}` (starts with RIS, replays ≤ `history` lines), then `Output{data}`, with optional `Resync{skipped, snapshot}` (does *not* reset, so client scrollback is kept), then `Exited`.
- **Client → daemon stream control:** `Input`, `Resize`, `Detach`, `Ack{consumed}` (cumulative stream bytes), `Ping{nonce}` → `Pong`.
- `StreamOpts{window, history}` per subscription: `LOCAL = 8 MiB / 5000 lines`, `REMOTE = 256 KiB / 2000 lines`.

### Flow control in detail

Per subscriber, the daemon tracks `sent` (stream bytes queued) and `acked`.

1. On each PTY chunk (holding the session's subscriber lock, which also orders snapshots): if `sent - acked + n > window`, mark the subscriber **lagging** and count `n` as skipped. Otherwise push the shared frame.
2. While lagging, no output goes to this subscriber, so its queue drains.
3. The client acks **whenever its transport read buffer is empty** and in between every `window/4`. The "buffer empty" rule guarantees the final partial window is acked, so a lagging subscriber can never deadlock waiting for an ack.
4. When `acked == sent` while lagging: send `Resync` (one screen + modes, about 2–20 KB) and resume streaming.

Effects:
- **Local:** the client parses at ≥175 MB/s with an 8 MiB window, so it never lags. Lossless, same as before.
- **Remote flood** (`cat 1GB.log` over a 2 MB/s link): bandwidth use is bounded, the screen shows current output every round, and **Ctrl+C → prompt visible ≈ window / bandwidth + RTT** (256 KiB at 2 MB/s ≈ 130 ms). With unbounded forwarding (plain ssh, or our Stage 0 queues) that is "after the whole backlog drained".
- **Many panes on one link:** worst-case head-of-line delay ≈ Σ windows / bandwidth. Bounded, and see R1 "interactive priority".
- **Memory:** daemon queues are bounded by Σ windows. Stage 0 had unbounded per-client queues, which a stalled SSH client could grow to gigabytes.
- **Cost of skipping:** lines skipped during a flood are missing from the *client's* scrollback. The daemon still has them (R1: lazy history fetch fills the gap on scroll-up).

Covered by `slow_client_is_skipped_forward_with_resync`: an 8 MB flood to a non-acking client delivers < 1 MB, and the subsequent `Resync` shows the final screen.

## 5. Transports

### Local
`$XDG_RUNTIME_DIR/goatty/goattyd.sock`, dir 0700, socket 0600. The client auto-starts `goattyd serve` with `setsid` if it's missing.

### SSH (implemented: `goatty --ssh <host> [--remote-daemon PATH] [--plain]`)
`Endpoint::Ssh` runs `ssh -T -e none -o ServerAliveInterval=10 -o ServerAliveCountMax=3 -o ConnectTimeout=10 -- <host> sh -c '…'` (`--` so a host can't be read as an option). The remote script prefers this build's installed goattyd (`~/.local/share/goatty/bin/goattyd-<version>-p<protocol>`), then `goattyd` in `PATH`, `~/.local/bin` or `~/.cargo/bin` (non-interactive SSH often lacks those in PATH), and execs `goattyd proxy`. Before that it prints one line to stderr, `goatty-remote: <uname -s> <uname -m> <path or ->`, which tells the client the host's platform and what it found without another round trip. It is POSIX sh because the login shell may be fish.

ssh's stderr is captured (logged, last 40 lines kept): a failed connect shows ssh's own message instead of "no greeting". Prompts are unaffected, since ssh asks on `/dev/tty` or through askpass, never on stderr.

`goattyd proxy` connects to the host's daemon socket, starting the daemon if needed, and relays bytes in both directions. It writes stdout through the raw fd, because std's `Stdout` is line-buffered and would hold binary frames back.

**Tailscale:** works out of the box with `--ssh <magicdns-name>`, using normal OpenSSH over the tailnet or Tailscale SSH. No goatty-specific network port is needed.

### Any stdio command (implemented: `goatty --via '<cmd>'`)
E.g. `--via 'docker exec -i devbox goattyd proxy'`. The tests use this with the local `goattyd proxy` to exercise the full remote path, including reconnect, without ssh.

### Socket watchdog (implemented)
If another listener has taken over the path (a second daemon, or a systemd socket installed meanwhile), the watchdog leaves it alone and says so once.

On servers, logind deletes `/run/user/$UID` when the last login session ends, unless linger is enabled. The daemon survives, but its socket path disappears, and the next `ssh host goattyd proxy` would silently start a second, empty daemon. goattyd checks the socket's inode every 2 s and re-binds if it changed. (R1 also recommends `loginctl enable-linger` and ships a systemd user unit.)

## 5a. goattyd vs. plain SSH (decided 2026-11-09, #5)

Connecting to a host always means goattyd on that host, unless the user picks plain SSH.

`goatty --ssh host` opens the terminal window right away; its tab shows the connect screen (`goatty-app/src/connect.rs`, #37) until the session is there, and so does every tab opened on a host. Failures goatty can fix become questions in that tab (`RemoteProblem`):

| Situation | Offered |
|---|---|
| No goattyd on the host, platform supported | **Install goattyd** · Plain SSH · Cancel |
| Platform without a goattyd build (macOS, BSD, armv7, …) | Plain SSH · Cancel |
| Supported, but this goatty carries no binary for it | Retry (after installing by hand) · Plain SSH · Cancel |
| Running goattyd older than our range | **Update and restart** (says it ends the host's sessions) · Plain SSH · Cancel |
| This build's goattyd installed, but an older daemon still runs | **Restart goattyd** (same warning) · Plain SSH · Cancel |
| Running goattyd newer than our range | Plain SSH · Cancel ("update goatty on this computer"); goatty never downgrades a daemon |
| Anything else (auth, network) | Retry · Plain SSH · Close, with ssh's last words |

**Install** (`remote::install`) streams the binary over the same ssh into `~/.local/share/goatty/bin/.goattyd-….$$`, checks the size, runs `--version` there (catches wrong CPU, too-old libc, `noexec` home), then renames it into place. Nothing else on the host changes: no sudo, no PATH or dotfile edits; `rm -r ~/.local/share/goatty` undoes it. Restarting runs that goattyd's `stop --force`, which stops a daemon of *any* version through its socket (`SO_PEERCRED`, only if the process is a `goattyd`; through systemd when it's the service). The binaries come from the release archive: `remote/goattyd-{x86_64,aarch64}-linux`, static musl builds (`release.yml`; measuring them against glibc: #132). Development builds upload their own `goattyd` when the host has the same CPU, with a note that it may not run on an older libc. After a successful install it deletes the other `goattyd-*` builds in that directory unless a running process uses one (`/proc/<pid>/exe`; without `/proc` it keeps them all), so updates don't pile up binaries.

**Plain SSH** (`--plain`, or the button) runs `ssh -- host` in a session of the local goattyd (`Run`, v6), falling back to a window-owned PTY if the local daemon is too old. You get exactly what `ssh` in any terminal gives: the user's ssh config, prompts in the terminal, the host's own shell. Nothing is installed, and nothing of goatty runs there (no managed shell, no host settings or plugins; the settings window configures the local host). The session survives closing the window (the ssh process lives in the local daemon), but not a network drop. Typing `ssh host` inside a goatty shell is the same thing and needs no feature at all.

Remembering the choice per host: the Hosts settings page sets `ssh.hosts[].plain`, and `goatty --ssh host` and the palette then go straight to plain SSH for that host.

**Host list (#115).** The command palette lists the hosts to connect to: the ones added in the settings (`ssh.hosts`) and, with `ssh.discover` (default on), every concrete `Host` in `~/.ssh/config` and the files it `Include`s (`goatty_client::ssh_config`, read off the UI thread into the app's `hosts::HostList`). In the full palette (Ctrl+Shift+P) hosts appear once something is typed, or when recently used, and picking one connects.

**Host switcher and hosts in tabs (#4, #37).** Ctrl+Shift+H (`connect_to_host`) lists this computer and the hosts, with `user@hostname · OS · RTT` (`Connection::host()`/`rtt()`) for the ones goatty is connected to, and offers to connect to any `user@host` typed there. Picking a host lists its sessions (`list_sessions`, connecting first when no tab is on it): a new session, or one to attach to. Plain SSH hosts connect right away. A host opens where `ssh.open_in` says: a window of its own (default) or a new tab of the active window (Shift+Enter in the session list: the other way; Settings → Hosts). Either way its tab shows the connect screen above, so a missing or old goattyd is handled the same way as with `--ssh`.

A window can hold tabs on several hosts. Each tab keeps its host: new tabs and splits from it open there (plain SSH tabs run `ssh host` again). Tabs on the same host share one `Connection` (`hosts::pool`, weak: it closes with its last tab), so a second tab doesn't log in again. Each goattyd keeps the layout of the window's tabs on it, so `goatty --ssh host` brings back that host's tabs. The settings window and "reload shells" act on the host of the active tab.

## 5b. SSH prompts in the GUI (#6) and goattyd as a systemd service (#7)

**Prompts.** For `Endpoint::Ssh` the GUI sets `SSH_ASKPASS` to its own binary, `SSH_ASKPASS_REQUIRE=force` (OpenSSH ≥ 8.4; also when started from a terminal) and `GOATTY_ASKPASS=1` (internal marker, not a setting). ssh then runs `goatty "<prompt>"`; `main` sees the marker and shows one small window (`goatty-app/src/askpass.rs`): a masked field for passphrases and passwords, "Trust and connect"/"Cancel" for unknown host keys (answers `yes`/`no`), Yes/No for `SSH_ASKPASS_PROMPT=confirm`, a notice for `none` (security key touch). Answer on stdout, verdict in the exit status. The prompt kind is decoded in `goatty_client::askpass`. Reconnects can prompt again (#130).

**systemd user service** (opt-in: `goattyd service install|uninstall|status`). `goattyd.socket` listens on `%t/goatty/goattyd.sock` (0600, dir 0700); `goattyd.service` runs `goattyd serve`, which takes fd 3 per `sd_listen_fds` and removes `LISTEN_*` so shells don't inherit them. Clients that find the socket missing start the socket unit (`systemctl --user start goattyd.socket`) when it's installed, otherwise a detached daemon as before. Decisions:

- **Opt-in, not the default.** Without lingering, systemd stops user services, and with them every session, when the last login session ends: on a server that is every SSH disconnect. The detached daemon survives that (unless logind's `KillUserProcesses=yes`). `install` reports the linger state and prints `loginctl enable-linger` instead of running it. On desktops, a systemd-started daemon gets the user manager's environment, not the GUI's (`SSH_AUTH_SOCK`, PATH additions), which shells notice.
- **No `Requires=goattyd.socket`** in the service: stopping or uninstalling the socket never takes sessions with it. `uninstall` leaves a running daemon alone.
- **Never hijack a running daemon:** `install` only starts the socket when no daemon started the old way is serving the path; otherwise systemd takes over after it stops.
- **FDSTORE** (shells surviving a daemon restart or upgrade) is designed but not built: #131.

## 6. Reconnect (implemented)

- The client's reader sees EOF/error, or the heartbeat sees 20 s of silence (Ping every 5 s on remote links). It closes the transport (kills the ssh child), marks panes `Disconnected`, and fails in-flight requests.
- Backoff: 0.2, 0.5, 1, 2, then every 5 s. Each attempt runs a new transport, handshake, and then `Attach` for every pane with its last size.
- The reattach snapshot starts with RIS, which also wipes the local "[goatty: connection lost; reconnecting…]" line the client printed into its mirror. The window title shows `— user@host (reconnecting…)`.
- `Attach` fails (e.g. the daemon restarted) → `Lost`.
- The UI never blocks: input/resize go through the writer thread. Input typed while disconnected is dropped, not replayed later (replaying stale keystrokes into a shell is dangerous).

Covered by `client_over_proxy_reconnects_and_reattaches`: kill the proxy mid-session → reconnect through a fresh proxy → same shell PID, shell variables intact, screen restored.

## 7. Performance

### Measured (`cargo run --release -p goatty-daemon --example netbench -- [proxy | slow RATE | ssh HOST]`)

Headless client feeding a real mirror VT (the GUI's work minus rendering), 200×50, `/bin/sh`, same machine as Stage 0:

| Endpoint | Echo p50 / p90 | 200 MB flood | Delivered | Resyncs |
|---|---|---|---|---|
| Local Unix socket (8 MiB window) | 0.57 / 0.99 ms | 0.83 s (242 MB/s) | 200.0 MB, lossless | 0 |
| `goattyd proxy` over stdio (256 KiB window: the remote code path) | 0.49 / 0.76 ms | 0.89 s (224 MB/s) | 199.9 MB | 1 |
| Same, throttled to **2 MB/s** with `pv -L` | 0.75 / 1.28 ms | **0.74 s** | 3.0 MB | 5 (197 MB skipped) |

Echo latency includes the PTY round trip and the benchmark's 0.2 ms polling. On the throttled link, naive forwarding would need ~100 s to drain the flood before the prompt (or a Ctrl+C) became visible. With skip-and-resync the prompt shows when the command finishes, and keystrokes stay fast during the flood. Local throughput is lossless and well above what the GUI renders (~100 MB/s end to end in Stage 0), so the protocol is not the bottleneck.

### Budget

| Path | Budget | Notes |
|---|---|---|
| Local flood throughput | ≥ Stage 0 | One encode per chunk shared by all subscribers, batched writes with flush-on-idle, acks only every ≥2 MiB or per drained read. Still to do: re-run the GUI `scripts/bench.sh` |
| Local keystroke → PTY | < 0.2 ms added | Writer thread flushes as soon as its queue is empty |
| Remote keystroke echo | RTT + ~1 ms | R3 adds prediction to hide the RTT |
| Remote attach | ≤ 2000 lines of history | ~100–300 KB uncompressed. R1: zstd plus lazy history |
| Remote flood: Ctrl+C visible | ≈ window/bandwidth + RTT | 256 KiB window |
| Daemon memory per subscriber | ≤ window | Was unbounded |
| Idle | 0 wakeups locally, 1 ping per 5 s remote | |

### Pitfalls found while building R0
- **`std::io::copy` from a pipe into a Unix socket uses `splice()` on Linux, and the data can stay stuck in the kernel.** In `goattyd proxy`, the client's Hello never reached the daemon, so the handshake deadlocked. The proxy now uses a plain read/write loop. This would have broken every SSH connection.
- std's `Stdout` is line-buffered, so the proxy writes through the raw fd.
- Over SSH, socket backpressure says nothing about client progress (ssh's channel window plus TCP buffers are megabytes). That is why flow control uses application-level acks.

## 8. Roadmap

### R0: protocol and transports ✅ (this change)
Protocol v2, `goatty-client` crate, multiplexed connections, ack/window flow control with resync, encode-once broadcast, `goattyd proxy`, `--ssh` / `--via`, reconnect with reattach, heartbeat/RTT, socket watchdog, tests for all of it.

### R1: remote MVP polish (next; tracked in the [R1 milestone](https://github.com/goatty-dev/goatty/milestone/1))
1. **Host list and switcher UI**: ✅ `~/.ssh/config` `Host` entries plus hosts added in the settings, in the command palette (#115, §5a); the host switcher with OS and RTT, a per-host session picker, and hosts as tabs of one window (#4, #37).
2. ✅ **Remote goattyd install and version negotiation**, plus the plain SSH fallback (§4, §5a).
3. ✅ **SSH prompts in the GUI** through goatty as `SSH_ASKPASS` (§5b).
4. ✅ **Daemon as systemd user service** (opt-in, socket-activated) with linger guidance (§5b). FDSTORE: #131.
5. **Interactive priority in the daemon writer**: small frames (echo, prompts) jump ahead of bulk frames from flooding panes. This removes most cross-pane head-of-line delay on one link.
6. **Adaptive window**: measure delivery rate from acks and RTT from pings, then set `window ≈ max(64 KiB, rate × 50 ms)`. A fast LAN streams losslessly, a slow link skips early.
7. **Lazy history**: `FetchHistory{session, before_line, count}` fills the client's scrollback on demand. It covers both the trimmed attach snapshot and lines skipped by resync. It needs stable line numbering (absolute line index since session start) in the snapshot.
8. **Compression** for snapshots and history (zstd, negotiated). Live output stays uncompressed (it's small, and latency matters more). Users can also set `Compression yes` in ssh_config.
9. **Size policy for multiple clients**: one *controlling* client per session sets the PTY size. Others watch (letterboxed) or take over explicitly. Today, last resize wins.
10. **Daemon-side liveness**: drop subscribers whose connection has been silent for >60 s (pings), so the attached count and size control are correct after a client vanished without FIN.
11. **Client vs host settings split** in the profile model (font/keys/theme are client; shell/plugins/env are host). The plugin manager always acts on the connected host.

### R2: native network transport (optional, later)
Only needed if SSH is the bottleneck or for zero-config pairing.
- **QUIC (quinn) + TLS** with one QUIC stream per pane. That removes head-of-line blocking between panes entirely, and connection migration covers laptop roaming (Wi‑Fi → LTE). The protocol already fits: frames are per-session, and the flow-control logic carries over.
- **Auth**: device pairing (a short code shown by `goattyd pair` → client cert pinned on both sides). Optionally Tailscale identity (`tailscale whois` on the peer address) when bound to the tailnet interface only.
- Bind to loopback/tailnet only by default, and never to 0.0.0.0 without explicit opt-in (plan §10).

### R3: latency hiding (optional, later)
Mosh-style **local echo prediction** using the client's mirror VT, which we already have. Show predicted printable characters underlined, then confirm or roll back when the daemon's output arrives. Only enable it when RTT > ~30 ms and the terminal isn't in the alt screen or a password prompt (echo off). Our architecture makes this an overlay on the client terminal, not a protocol change.

## 9. Risks and open questions

| Risk | Mitigation |
|---|---|
| Version skew between GUI and remote daemon | Version range negotiation; install/restart from the connect screen (§5a) |
| Head-of-line blocking across panes on one SSH stream | Bounded by Σ windows; R1 interactive priority; R2 QUIC streams |
| Scrollback gaps after resync | R1 lazy history from the daemon's authoritative scrollback |
| Security: proxy access = full control of the daemon | Same trust as an SSH shell on that host; the daemon socket stays 0600 and is never exposed |
| `$XDG_RUNTIME_DIR` removed on logout | Watchdog re-bind (done); systemd user unit + linger (done, opt-in) |
| `goattyd` not in PATH for non-interactive SSH | Script searches common dirs; `--remote-daemon`; auto-install (done) |
| Windows client | `Endpoint::Command` with `ssh.exe` works unchanged; local transport needs named pipes instead of Unix sockets, and process detaching instead of `setsid` |
| Session size conflicts with two clients | R1 controlling-client policy |

## 10. How to try it

```sh
cargo build --release --workspace
# goatty offers to install goattyd on the server if it's missing or too old
# (or install it yourself: cargo install --path crates/goatty-daemon → ~/.cargo/bin/goattyd)

./target/release/goatty --ssh myserver          # ~/.ssh/config alias, user@host, or a Tailscale name
./target/release/goatty --ssh myserver --plain  # just `ssh myserver` in a local session
goattyd service install                         # on a server: systemd user service (+ loginctl enable-linger)
./target/release/goatty --ssh myserver --new    # another session on that host
./target/release/goatty --via 'docker exec -i devbox goattyd proxy'

# Reconnect: start `htop` remotely, then toggle Wi‑Fi or `pkill -f 'goattyd proxy'` on the server.
# Persistence: close the window, then run `goatty --ssh myserver` again → same session.
```
