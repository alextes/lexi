---
schema_version: 6
id: lexi-obv6
title: Revamp admin tool/session architecture
priority: P2
status: todo
type: meta
deps: []
owner: null
created_at: 2026-01-09T11:30:13.856142Z
updated_at: 2026-01-09T11:30:13.856142Z
---

Meta-issue: rework admin tool/session architecture. Split into dependent tasks for storage, gating, admin tools, markers/timeout, and tests.

Decisions:
- Admin-mode marker should be a response prefix.
- Admin-only tools defined via a static list in code (not config).