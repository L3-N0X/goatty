---
title: Daemon-side liveness: drop silent subscribers
status: open
labels: [daemon,remote]
milestone: R1
created: 2026-11-07
---

A client that vanished without FIN (dead TCP behind ssh) stays subscribed until the proxy dies. Track last activity per connection (pings arrive every 5 s from remote clients) and drop connections silent for >60 s, so the `attached` count and size control stay correct.
