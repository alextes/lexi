# Instructions for AI agents

## check, lint, test

before committing anything, or when finishing a big chunk of work, consider running:

- `cargo clippy`
- `cargo test`
- `carge fmt --all`

## commits

this repo uses conventional commits (e.g., `feat: ...`, `fix: ...`, `refactor(bot): ...`).

<!-- braid:agents:start v7 -->
## braid workflow

this repo uses braid (`brd`) for issue tracking. issues live in `.braid/issues/` as markdown files.

basic flow:
1. `brd start` — claim the next ready issue
2. do the work, commit as usual
3. `brd done <id>` — mark the issue complete
4. ship your work:
   - in a worktree: `brd agent merge` (rebase + ff-merge to main)
   - on main: just `git push` (you're already there)

useful commands:
- `brd ls` — list all issues
- `brd ready` — show issues with no unresolved dependencies
- `brd show <id>` — view issue details (shows deps and dependents)
- `brd show <id> --context` — view issue with full content of related issues
- `brd config` — show current workflow configuration

**tip:** before starting work, use `brd show <id> --context` to see the issue plus all its dependencies and dependents in one view.

## working on main vs in a worktree

**quick check — am i in a worktree?**

```bash
cat .braid/agent.toml 2>/dev/null && echo "yes, worktree" || echo "no, main"
```

**if you're in a worktree (feature branch):**
- `brd start` handles syncing automatically
- use `brd agent merge` to ship (rebase + ff-merge to main)
- if you see schema mismatch errors, rebase onto latest main

**if you're on main:**
- `brd start` syncs and claims
- after `brd done`, just `git push` your code commits
- no `brd agent merge` needed — you're already on main

## design and meta issues

**design issues** (`type: design`) require human collaboration:
- don't close autonomously — discuss with human first
- research options, write up trade-offs in the issue body
- produce output before closing (implementation issues or a plan)
- only mark done after human approves

**meta issues** (`type: meta`) are tracking issues:
- group related work under a parent issue
- show progress as "done/total" in `brd ls`
- typically not picked up directly — work on the child issues instead

## syncing issues (issues with code)

this repo stores issues **with code** — issues live in `.braid/issues/` and sync via git.

**how it works:**
- `brd start` auto-syncs: fetches, rebases, claims, commits, and pushes
- `brd done` marks complete and auto-pushes (if auto_push enabled)
- issue changes flow through your normal git workflow

**in a worktree (feature branch):**
```bash
brd done <id>        # marks done, auto-pushes issue state
brd agent merge      # ship code to main (rebase + ff-merge)
```

**on main:**
```bash
brd done <id>        # marks done, auto-pushes issue state
git push             # push your code commits
```

**changing settings:**
- `brd config` — show current config
- `brd config issues-branch <name>` — enable issues-branch for multi-agent setups
<!-- braid:agents:end -->
