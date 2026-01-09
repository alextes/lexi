---
schema_version: 6
id: lexi-rviq
title: Refactor admin session logic into module
priority: P2
status: todo
deps: []
owner: null
created_at: 2026-01-09T11:30:14.355942Z
updated_at: 2026-01-09T11:30:14.355942Z
---

Extract admin session handling from src/bot/mod.rs into its own module to keep the bot update flow focused on orchestration. Move helpers like admin session resolution, timeout handling, response-id restoration, and admin-mode prefixing into a dedicated file with clear interfaces. Update src/bot/mod.rs to call the new module, and add any targeted tests if needed.