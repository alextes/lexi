---
schema_version: 6
id: lexi-jedz
title: Add tool to retrieve recent conversation history
priority: P2
status: todo
deps: []
owner: null
created_at: 2026-01-09T11:30:14.113454Z
updated_at: 2026-01-09T11:30:14.113454Z
acceptance:
- Tool is DB-backed via Db::get_message_history with parameter n (default 10)
- Tool is available in all sessions (not admin-only)
- Output schema returns messages ordered oldest→newest and is model-friendly (role + content, optional metadata)
- Handles empty history and invalid n gracefully (documented behavior)
---

Add a tool that returns the last n conversation messages for model context (default n=10).

Notes: Testing: add unit tests for argument parsing (default n=10, invalid/negative/clamped), DB call integration with MockDb to assert limit and ordering, and output schema (roles/content).