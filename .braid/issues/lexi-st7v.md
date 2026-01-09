---
schema_version: 6
id: lexi-st7v
title: Tests for admin session gating and lifecycle
priority: P2
status: todo
deps:
- lexi-obv6
owner: null
created_at: 2026-01-09T11:30:13.986278Z
updated_at: 2026-01-09T11:30:45.096992Z
acceptance:
- Unit tests updated/added for tool gating and session lifecycle
- Timeout behavior validated (auto-end + notification)
- Tests pass
---

Update/add tests covering admin tool gating, session start/end, admin-mode marker, and timeout behavior.