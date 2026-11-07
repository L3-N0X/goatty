---
title: Ship goattyd as a systemd user service (+ linger docs, FDSTORE)
status: open
labels: [daemon,packaging]
milestone: R1
created: 2026-11-07
---

Provide a systemd user unit and socket activation, and document `loginctl enable-linger` for servers. Design in the FDSTORE idea from `docs/stage0-findings.md` §4 (PTY fds survive daemon restarts/upgrades). The socket watchdog stays as a fallback.
