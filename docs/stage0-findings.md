# Stage 0 findings: GPUI + rio-vt spike

**Date:** 2026-11-07 · **Machine:** CachyOS, niri (Wayland), RTX 4070 Laptop (Vulkan), 28 threads, 144 Hz panel

**Verdict:** GPUI + rio-vt works. It is fast enough and correct enough to continue. The daemon/reattach model is already working too, so part of Stage 2 has been pulled forward.

---

## What was built

| Crate | Role |
|---|---|
| `goatty-term` | The "TerminalEngine". It is the only crate that touches rio-vt: frame snapshot with RGB-resolved cells, palette, key/mouse/paste encoding, PTY (portable-pty), reattach serialization |
| `goatty-proto` | Unix-socket protocol (length-prefixed postcard) |
| `goatty-daemon` (`goattyd`) | Owns the PTYs and an authoritative headless VT per session. Accepts attach/detach. CLI: `list`, `send`, `kill`, `stop` |
| `goatty-app` (`goatty`) | GPUI client: custom grid `Element`, input, selection, clipboard, scrollback, mouse reporting |

## Measurements

### VT core (headless, `cargo run --release -p goatty-term --example vtbench`)

| Workload | Throughput |
|---|---|
| Plain ASCII | ~487 MB/s |
| Heavy SGR (256 + truecolor) | ~176 MB/s |
| CJK / emoji / combining | ~84 MB/s |
| Frame snapshot (200×60, under lock) | ~0.24 ms |

rio-vt is not the bottleneck: these numbers are above what a PTY delivers.

### End-to-end in the window (`scripts/bench.sh`, same workload in every terminal)

| Terminal | `seq 1 3000000` (23 MB) | Colored flood (100 MB) |
|---|---|---|
| **goatty** (glyph-cache renderer) | **1.28 s** | **1.00 s → 100 MB/s** |
| alacritty (user config) | 1.32 s | 1.35 s → 74 MB/s |
| kitty (user config) | 1.66 s | 1.23 s → 82 MB/s |

These are single runs, so expect about ±15% noise. The other terminals run the user's configs. goatty does less work than they do so far (no ligatures, no URL detection), so read this as "same league", not "faster".

### Frame cost during the flood (`GOATTY_STATS=1`)

(Measured on gpui 0.2.2/blade. After the switch to gpui git/wgpu, throughput was re-checked at 113 MB/s; fps still needs a focused re-run.)

| Renderer | Prepaint | Paint | fps while ingesting 60–90 MB/s |
|---|---|---|---|
| Per-segment line shaping (`GOATTY_RENDER=shaped`) | 10–17 ms | 0.4 ms | 43–51 |
| **Per-cell glyph cache (default)** | **0.3–0.5 ms** | 0.3–1.6 ms | **123–145 (vsync-capped)** |

An idle terminal renders 0 frames. With htop refreshing, the GUI uses about 3% CPU.

### Reconnect

Automated tests (`cargo test -p goatty-daemon`) and a manual GUI check both pass:
- Close the GUI, and the shell keeps running (same PID). Output written while detached appears after reattach. Shell variables survive.
- A full-screen app (htop) is restored exactly on reattach, including alt-screen and DECCKM state.
- The GUI auto-starts `goattyd` (detached with `setsid`) when it isn't running.

---

## Architecture recommendations (changes vs. the plan)

### 1. Renderer: use a per-cell glyph cache, not text shaping
Shaping row text with gpui's `shape_line` works, but every scrolled line misses the cache. That costs 10–17 ms per frame, which is too slow for 120/144 Hz. Instead, resolve each `(char, bold, italic)` to `(font_id, glyph_id)` once (gpui's shaper still handles font fallback) and call `window.paint_glyph` at grid positions. Prepaint then drops to under 0.5 ms.
- **Trade-off:** no programming ligatures in that mode. Keep line shaping as an opt-in "ligatures" setting (already implemented as `GOATTY_RENDER=shaped`).
- **Next:** draw box-drawing and block elements (U+2500–259F) procedurally. With font glyphs they misalign at a 1.25 line height (visible in the smoke test). Every serious terminal does this.

### 2. Session model: authoritative VT in the daemon, mirror VT in the client
The plan left open whether the daemon or the GUI owns terminal state. Implemented answer: **both run rio-vt**.
- The **daemon's** VT is authoritative and is the **only one that answers terminal queries** (DA, DSR, OSC 4/10/11). If both answered, the shell would see every reply twice. This is the `answer_queries` flag.
- On attach, the daemon sends a **VT snapshot**: replayed scrollback, alt-screen switch, screen contents, and input modes (DECCKM, bracketed paste, mouse modes, keypad, title). After that it streams raw bytes, with no gap or overlap: snapshot and subscribe happen under one lock.
- The client feeds bytes into its own VT and renders from it. So `--local` (in-process PTY) and daemon mode share 100% of the UI code.
- **Cost:** bytes get parsed twice. At over 175 MB/s per parse, that doesn't matter.
- **Rejected alternative:** the daemon sends rendered grid diffs (tmux control-mode or zellij style). That means more protocol, client-side scrollback becomes awkward, and nothing is gained locally.

### 3. One socket connection per pane (superseded: see remote-architecture.md D4, connections now multiplex sessions)
Each pane is its own connection, so flow control and backpressure are per pane for free. Control commands (`list`/`kill`/`send`) use short-lived connections. Multiplexing only becomes worthwhile for the later remote/web API.

### 4. Survive daemon upgrades: systemd fd store (new idea)
The plan says sessions die with the daemon. On Linux, `goattyd` as a systemd **user** service can hand its PTY master fds to systemd's **FDSTORE** before restarting, then pick them up again. Shells then survive daemon upgrades and crashes (the daemon re-creates its VT from a persisted snapshot). Worth designing in early (Stage 2), because it constrains how session state is serialized.

### 5. Have a VT fallback ready: `alacritty_terminal` is the better candidate than `wezterm-term`
Zed's own terminal is GPUI + `alacritty_terminal`, the most battle-tested combination for exactly this stack, and it's published on crates.io. `wezterm-term` is only available as third-party forks on crates.io. rio-vt has been fine so far and its API is very close to alacritty's (Crosswords is derived from it), so swapping behind `goatty-term` would be cheap.

### 6. Threads, not async, for now
One reader thread per session and one per connection is simple and fast. tokio adds nothing until the remote API arrives.

---

## Problems found

| Issue | Status / mitigation |
|---|---|
| **Window froze after the first frames** (static "image" while input still reached the shell). Log: `blade_graphics … Acquire failed because the surface is out of date`. Cause: crates.io gpui 0.2.2's blade renderer never recreates the Vulkan swapchain after `VK_ERROR_OUT_OF_DATE_KHR`, which niri triggers when it resizes a new window into its tile | **Fixed** by moving to gpui from Zed git (pinned rev `b5cd310`, 2026-11-07). The Linux renderer there is `gpui_wgpu`; resize and live updates verified |
| gpui is now a git dependency: `gpui` + `gpui_platform` (the app starts with `gpui_platform::application()`) | Bump on purpose: change the rev in the root `Cargo.toml`, then `cargo update -p gpui`. Zed's toolchain is 1.98.1+ |
| ~~libc pin~~: needed only for gpui 0.2.2's `xattr` dependency | Removed with the gpui switch |
| Startup to first window took 2–3 s in my log timestamps (old blade renderer) | The user didn't notice any slowness. Re-measure on wgpu before acting |
| Unfocused/covered windows render ~1 fps during heavy output | Most likely compositor frame-callback throttling. Expected behavior, but check it's not a gpui_wgpu regression with a focused benchmark |
| GUI RSS about 220 MB (mostly GPU driver mappings) | Monitor; typical for Vulkan apps on NVIDIA |
| rio-vt `contents_formatted()` covers only the visible screen and drops combining marks | goatty's snapshot adds scrollback (up to 5,000 lines) and modes. The alt screen's hidden primary screen can't be restored (rio keeps it private) |

## Not done yet (Stage 1 backlog)
- IME / compose (needs gpui `InputHandler`), kitty keyboard protocol, cursor blink
- Procedural box-drawing glyphs; ligature setting
- Selection auto-scroll, URL/path detection, OSC 133/7 integration
- Multiline paste confirmation (plan §9)
- Tabs/splits (one pane per window right now)
- Not yet checked in a real session: Neovim, tmux-in-goatty, mouse-heavy TUIs (htop rendering confirmed)
