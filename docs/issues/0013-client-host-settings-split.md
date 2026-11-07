---
title: Split client vs host settings in the profile model
status: open
labels: [config,remote]
milestone: Stage 4
created: 2026-11-07
---

Font/keys/theme/clipboard are client settings; shell/plugins/tools/env/profiles are host settings stored by goattyd. The plugin manager always acts on the connected host. See `remote-goattyd-plan.md` §5.
