---
schema_version: 6
id: lexi-ec4i
title: Refactor handle_telegram_update for readability
priority: P2
status: todo
deps: []
owner: null
created_at: 2026-01-09T11:30:14.233747Z
updated_at: 2026-01-09T11:30:14.233747Z
acceptance:
- handle_telegram_update reads as high-level flow with 3-6 helper calls
- Helpers are unit-tested where sensible (pure logic such as prompt extraction/admin gating)
- No behavior changes; tests pass
---

Break up src/bot/mod.rs handle_telegram_update into smaller helpers focused on discrete steps: message persistence, prompt extraction/admin gating, AI invocation, outcome handling, and reply sending. Consider introducing a small per-update context struct to reduce argument threading.