# Remote goattyd: architecture and plan

**Date:** 2026-11-07 · **Status:** R0 implemented in the prototype (protocol v2, SSH/stdio transports, flow control, reconnect). R1 onward is planned.

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

- **Handshake:** `Hello{version, client}` → `Hello{version, daemon_pid, host: HostInfo}`. Exact version match for now; a mismatch fails with a clear message.
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

### SSH (implemented: `goatty --ssh <host> [--remote-daemon PATH]`)
Runs `ssh -T -e none -o ServerAliveInterval=10 -o ServerAliveCountMax=3 -o ConnectTimeout=10 <host> sh -c '…'`. The remote script finds `goattyd` in `PATH`, `~/.local/bin` or `~/.cargo/bin` (non-interactive SSH often lacks those in PATH), then execs `goattyd proxy`. It is POSIX sh because the login shell may be fish.

`goattyd proxy` connects to the host's daemon socket, starting the daemon if needed, and relays bytes in both directions. It writes stdout through the raw fd, because std's `Stdout` is line-buffered and would hold binary frames back.

**Tailscale:** works out of the box with `--ssh <magicdns-name>`, using normal OpenSSH over the tailnet or Tailscale SSH. No goatty-specific network port is needed.

### Any stdio command (implemented: `goatty --via '<cmd>'`)
E.g. `--via 'docker exec -i devbox goattyd proxy'`. The tests use this with the local `goattyd proxy` to exercise the full remote path, including reconnect, without ssh.

### Socket watchdog (implemented)
On servers, logind deletes `/run/user/$UID` when the last login session ends, unless linger is enabled. The daemon survives, but its socket path disappears, and the next `ssh host goattyd proxy` would silently start a second, empty daemon. goattyd checks the socket's inode every 2 s and re-binds if it changed. (R1 also recommends `loginctl enable-linger` and ships a systemd user unit.)

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
1. **Host list and switcher UI**: parse `~/.ssh/config` `Host` entries plus "Add host". Show user@host, OS and RTT, plus a sessions-per-host picker (`list_sessions` already exists).
2. **Remote goattyd install and version match**: if `Hello` fails or the version differs, offer to upload the matching static goattyd (`x86_64`/`aarch64` musl) to `~/.local/share/goatty/bin/goattyd-<ver>` over the same ssh and use it. Like VS Code Server, this makes "it just works" possible for new-to-Linux users. Needs version-range negotiation instead of exact match.
3. **SSH prompts in the GUI**: set `SSH_ASKPASS` + `SSH_ASKPASS_REQUIRE=force` pointing at a tiny goatty askpass helper. Today prompts only work when goatty was launched from a terminal.
4. **Daemon as systemd user service** plus docs on `loginctl enable-linger` for servers. Integrates with the FDSTORE idea from Stage 0 (shells survive daemon upgrades).
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
| Version skew between GUI and remote daemon | Exact match today, clear error. R1: version range plus auto-upload |
| Head-of-line blocking across panes on one SSH stream | Bounded by Σ windows; R1 interactive priority; R2 QUIC streams |
| Scrollback gaps after resync | R1 lazy history from the daemon's authoritative scrollback |
| Security: proxy access = full control of the daemon | Same trust as an SSH shell on that host; the daemon socket stays 0600 and is never exposed |
| `$XDG_RUNTIME_DIR` removed on logout | Watchdog re-bind (done); systemd user unit + linger (R1) |
| `goattyd` not in PATH for non-interactive SSH | Script searches common dirs; `--remote-daemon`; R1 auto-install |
| Windows client | `Endpoint::Command` with `ssh.exe` works unchanged; local transport needs named pipes instead of Unix sockets, and process detaching instead of `setsid` |
| Session size conflicts with two clients | R1 controlling-client policy |

## 10. How to try it

```sh
cargo build --release --workspace
# on the server (same version!): cargo install --path crates/goatty-daemon  → ~/.cargo/bin/goattyd

./target/release/goatty --ssh myserver          # ~/.ssh/config alias, user@host, or a Tailscale name
./target/release/goatty --ssh myserver --new    # another session on that host
./target/release/goatty --via 'docker exec -i devbox goattyd proxy'

# Reconnect: start `htop` remotely, then toggle Wi‑Fi or `pkill -f 'goattyd proxy'` on the server.
# Persistence: close the window, then run `goatty --ssh myserver` again → same session.
```
