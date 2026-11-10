---
paths:
  - "crates/goatty-proto/**"
  - "crates/goatty-client/**"
  - "crates/goatty-daemon/**"
---
# Wire protocol, client, daemon

- The wire carries raw VT bytes and VT-byte snapshots, never rio-vt types. `Attached` snapshots start with RIS. `Resync` snapshots must not reset, because the client keeps its scrollback.
- Flow control is part of the protocol. Clients `Ack` the bytes they consumed. Once a client is past `window`, the daemon skips output and later sends `Resync`. The local window is 8 MiB (lossless) and the remote window 256 KiB.
- Compatibility rules:
  - `Hello` stays variant 0, with `version` as its first field in both enums.
  - Peers negotiate a version in `MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION`.
  - The enums are append-only. Add new variants at the end, never change existing ones, bump `PROTOCOL_VERSION` and mark the variant `vN:`.
  - Neither side sends a message newer than the agreed version.
  - Raise `MIN_PROTOCOL_VERSION` only for a change that can't be appended.
  - Plugin operations go through `goatty_plugin::api` JSON, shell setup and diagnostics through `goatty_shell::setup` JSON (`Shell`, v9). New operations there need no version bump.
- Encode a broadcast frame once and share it as `Arc<[u8]>`.
- Don't use `std::io::copy` between pipes and Unix sockets. On Linux it uses `splice()`, which stalled the proxy. Use plain read/write loops, and write binary data to stdout through the raw fd.
- Remote hosts:
  - goattyd is the default and plain SSH the only fallback. Don't add a third mode.
  - Change nothing on a host outside `~/.local/share/goatty`, and install only after the user agreed.
  - Never restart a daemon (which ends its sessions) without telling the user.
- The e2e tests in `crates/goatty-daemon/tests/` each use their own `GOATTY_SOCKET` and a sandboxed `HOME`.
