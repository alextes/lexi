# Instructions for AI agents

## check, lint, test

before committing anything, or when finishing a big chunk of work, consider running:

- `cargo clippy`
- `cargo test`
- `carge fmt --all`

## issue tracking

this project uses **bd (beads)** for issue tracking.
run `bd prime` for workflow context, or install hooks (`bd hooks install`) for auto-injection.

**Quick reference:**

- `bd ready` - find unblocked work
- `bd create "Title" --type task --priority 2` - create issue
- `bd close <id>` - complete work
- `bd sync` - sync with git (run at session end)

for full workflow details: `bd prime`
