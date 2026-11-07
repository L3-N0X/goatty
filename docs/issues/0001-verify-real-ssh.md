---
title: Verify the remote path against a real SSH host
status: open
labels: [remote,testing]
milestone: R1
created: 2026-11-07
---

The R0 remote path is tested end to end through `goattyd proxy` over stdio (`client_over_proxy_reconnects_and_reattaches`), but never through real `ssh` (localhost isn't in known_hosts on the dev machine).

**To do**
- `goatty --ssh <host>` against a server/Tailscale node with goattyd installed in `~/.cargo/bin`.
- Check: PATH lookup script, `-e none`/`-T`, reconnect after toggling Wi-Fi, `pkill -f 'goattyd proxy'` on the server, close/reopen → same session.
- `cargo run --release -p goatty-daemon --example netbench -- ssh <host>` and add the numbers to `docs/remote-architecture.md` §7.
