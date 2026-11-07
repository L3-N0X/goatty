# goatty

A batteries-included terminal for people new to Linux: a GPU-rendered terminal, a managed Zsh setup, and sessions that survive closing the window.

> **Status: early development (Stage 1).** See [`docs/goatty-project-plan.md`](docs/goatty-project-plan.md) for the plan, [`docs/architecture.md`](docs/architecture.md) for the code structure, [`docs/stage0-findings.md`](docs/stage0-findings.md) for what the spike measured, [`docs/remote-architecture.md`](docs/remote-architecture.md) for connecting to goattyd on other machines, and [GitHub issues](https://github.com/L3-N0X/goatty/issues) for open work.

## Layout

```
crates/
  goatty-term/    terminal engine: rio-vt wrapper, frame snapshots, palette, input encoding, PTY, reattach snapshots
  goatty-proto/   goatty ⇄ goattyd wire protocol (transport-independent)
  goatty-client/  connection to a goattyd: local socket, ssh, any stdio command; reconnect; blocking Control client
  goatty-config/  settings: typed defaults + GOATTY_* overrides (Stage 4 profiles plug in here)
  goatty-daemon/  goattyd library + binary: owns PTYs and sessions
  goatty-app/     GPUI desktop client (bin: goatty)
docs/             plan, architecture, findings, remote design
scripts/          smoke.sh (visual test), perf.sh + bench.sh (throughput)
```

See [`docs/architecture.md`](docs/architecture.md) for how the crates and modules fit together and where upcoming work goes.

## Run

```sh
cargo build --release --workspace

./target/release/goatty            # attach to newest detached session (auto-starts goattyd)
./target/release/goatty --new      # new session
./target/release/goatty --local    # no daemon, PTY owned by the window
./target/release/goatty --ssh host # goattyd on another machine (needs goattyd installed there)
./target/release/goatty --via 'docker exec -i box goattyd proxy'
./target/release/goatty --help

./target/release/goattyd list      # sessions
./target/release/goattyd send 1 'htop\r'
./target/release/goattyd stop
```

To try reattaching: run something in goatty, close the window, start `goatty` again. Over `--ssh`, the client also reconnects by itself when the link drops (title shows `(reconnecting…)`).

Environment variables: `GOATTY_FONT`, `GOATTY_FONT_SIZE`, `GOATTY_SHELL`, `GOATTY_SOCKET`, `GOATTY_STATS=1` (prints per-second frame stats to stderr), `GOATTY_RENDER=shaped` (ligature-capable line shaping instead of the glyph cache).

Keys: `Ctrl+C` copies when text is selected and sends an interrupt otherwise. `Ctrl+V` / `Ctrl+Shift+V` paste, `Ctrl+Shift+C` copies, `Shift+PgUp/PgDn` scroll back, `Ctrl+Shift+=/-` change the font size.

## Test & benchmark

```sh
cargo test --workspace                                   # unit + daemon reconnect tests
cargo run --release -p goatty-term --example vtbench     # headless VT throughput
cargo run --release -p goatty-daemon --example netbench  # client⇄daemon echo latency + flood (args: proxy | slow 2m | ssh HOST)
GOATTY_SHELL=$PWD/scripts/smoke.sh ./target/release/goatty --local
FLOOD=/path/to/big.txt scripts/bench.sh goatty ./target/release/goatty --local
```

**Note:** gpui comes from Zed's git repo, pinned to a revision in the root `Cargo.toml`. To upgrade, change the `rev` and run `cargo update -p gpui`.
