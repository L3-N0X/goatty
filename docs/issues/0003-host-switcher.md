---
title: Host list and switcher UI
status: open
labels: [remote,app,ux]
milestone: R1
created: 2026-11-07
---

Parse `Host` entries from `~/.ssh/config` and let the user add hosts. Show user@host, OS and RTT (`Connection::host()`, `Connection::rtt()`), plus a per-host session picker (`list_sessions` exists). Depends on tabs/panes (0029).
