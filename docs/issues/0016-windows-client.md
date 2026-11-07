---
title: Windows client support
status: open
labels: [platform,client]
milestone: later
created: 2026-11-07
---

`Endpoint::Command` with `ssh.exe` should work unchanged. The local transport needs named pipes instead of Unix sockets, and process detaching instead of `setsid`. ConPTY for a local Windows daemon.
