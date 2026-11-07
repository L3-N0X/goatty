---
title: SSH prompts (passphrase, host key) inside the GUI
status: open
labels: [remote,app,ux]
milestone: R1
created: 2026-11-07
---

ssh prompts only work when goatty was started from a terminal (stderr/tty inherited). Set `SSH_ASKPASS` + `SSH_ASKPASS_REQUIRE=force` to a small goatty askpass helper that shows a GPUI dialog.
