# Issues

Open work is tracked here, one file per issue, until the repo has a GitHub remote (then they move to GitHub issues: `gh issue create --title … --body-file …`).

**Format:** `NNNN-slug.md` with front matter (`title`, `status: open|done|wontfix`, `labels`, `milestone`, `created`). When an issue is closed, set `status: done`, add a line saying what resolved it, and update this table.

| # | Title | Milestone | Labels |
|---|---|---|---|
| [0001](0001-verify-real-ssh.md) | Verify the remote path against a real SSH host | R1 | remote,testing |
| [0002](0002-gui-smoke-after-protocol-v2.md) | Smoke-test the GUI after the protocol v2 / goatty-client switch | R1 | app,testing |
| [0003](0003-host-switcher.md) | Host list and switcher UI | R1 | remote,app,ux |
| [0004](0004-remote-auto-install.md) | Auto-install a matching goattyd on remote hosts + version-range negotiation | R1 | remote,protocol |
| [0005](0005-ssh-askpass.md) | SSH prompts (passphrase, host key) inside the GUI | R1 | remote,app,ux |
| [0006](0006-daemon-systemd-unit.md) | Ship goattyd as a systemd user service (+ linger docs, FDSTORE) | R1 | daemon,packaging |
| [0007](0007-interactive-priority.md) | Interactive priority in the daemon's per-connection writer | R1 | daemon,performance,remote |
| [0008](0008-adaptive-window.md) | Adaptive flow-control window | R1 | daemon,client,performance,remote |
| [0009](0009-lazy-history.md) | Lazy scrollback fetch from the daemon | R1 | protocol,daemon,client |
| [0010](0010-snapshot-compression.md) | zstd compression for snapshots/history | R1 | protocol,performance,remote |
| [0011](0011-controlling-client.md) | Size policy for multiple clients on one session | R1 | daemon,protocol |
| [0012](0012-daemon-liveness.md) | Daemon-side liveness: drop silent subscribers | R1 | daemon,remote |
| [0013](0013-client-host-settings-split.md) | Split client vs host settings in the profile model | Stage 4 | config,remote |
| [0014](0014-quic-transport.md) | Native QUIC/TLS transport with device pairing | R2 (later) | remote,protocol |
| [0015](0015-echo-prediction.md) | Mosh-style local echo prediction | R3 (later) | remote,app,performance |
| [0016](0016-windows-client.md) | Windows client support | later | platform,client |
| [0017](0017-via-quoting.md) | `--via` splits on whitespace | R1 | client,ux |
| [0018](0018-test-tempdir-cleanup.md) | Integration tests and netbench leave temp dirs in /tmp | R1 | testing |
| [0019](0019-rio-log-noise.md) | Silence rio-vt warnings for unhandled OSC (e.g. systemd OSC 3008) | Stage 1 | term,polish |
| [0020](0020-ime-compose.md) | IME / compose input | Stage 1 | app,input |
| [0021](0021-kitty-keyboard.md) | Kitty keyboard protocol | Stage 1 | term,input |
| [0022](0022-cursor-blink-config.md) | Cursor style/blink settings | Stage 1 | app,config |
| [0023](0023-box-drawing.md) | Procedural box-drawing and block glyphs (U+2500–259F) | Stage 1 | app,rendering |
| [0024](0024-ligature-setting.md) | Ligature setting (shaped renderer opt-in) | Stage 1 | app,rendering,config |
| [0025](0025-selection-autoscroll.md) | Selection auto-scroll while dragging past the edge | Stage 1 | app,input |
| [0026](0026-url-path-detection.md) | Clickable URL and file path detection | Stage 1 | app,ux |
| [0027](0027-shell-integration-osc.md) | OSC 133 / OSC 7 shell integration | Stage 3 | term,shell |
| [0028](0028-multiline-paste-confirm.md) | Multiline / suspicious paste confirmation | Stage 1 | app,ux,security |
| [0029](0029-tabs-and-splits.md) | Tabs and split panes | Stage 1 | app |
| [0030](0030-tui-compat-check.md) | Verify Neovim, tmux-in-goatty and mouse-heavy TUIs | Stage 1 | term,testing |
| [0031](0031-fps-rerun-wgpu.md) | Re-measure fps, startup time and unfocused-window throttling on gpui_wgpu | Stage 1 | app,performance |
| [0032](0032-alt-screen-primary-restore.md) | Reattach can't restore the primary screen hidden behind the alt screen | later | term,daemon |
