---
title: Native QUIC/TLS transport with device pairing
status: open
labels: [remote,protocol]
milestone: R2 (later)
created: 2026-11-07
---

Only if SSH turns out to be limiting. quinn + TLS, one QUIC stream per pane (no cross-pane head-of-line blocking), connection migration for roaming. Auth via device pairing (`goattyd pair`) or Tailscale identity. Bind to loopback/tailnet only. Requires tokio in the client/daemon networking layer.
