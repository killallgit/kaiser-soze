# kaiser-soze

kaiser-soze is one Rust crate with one binary, `soze`: a local proxy that sits between a coding
agent and its model API, forwards every byte as it arrives, and taps a copy of the stream into
events. Each tool is an `Adapter`. Codex and Claude Code are implemented.

Read [docs/objective.md](docs/objective.md) first: it is the whole scope. The target data shape
is in [docs/common-format.md](docs/common-format.md).

## Gate

```
task check
```

It runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`, and
names every check that failed. To test a single module, run `task test:unit -- <module>::`.

## Rules

- **Never in the way.** Forwarding never waits on the tap. A slow or failing parser drops and
  counts; it never delays, alters or breaks the bytes the agent receives.
- **Tests first.** Write the failing test, then the code. Fixtures are synthetic, never captured
  real traffic.
- **No log or error text in tests.** Assert on types, values and events, never on message strings.
- **Data safety.**
  - Message content is stored in tap output but never printed in a terminal, chat log, test
    output or commit. Report counts and ids only.
  - Credential headers are redacted before anything is written.
  - Never open credential stores (`~/.codex/auth.json`, Cursor `ItemTable`, keychains).
  - Never write under agent folders (`~/.claude`, `~/.codex`, `~/.cursor`).
  - Never install the CA into a trust store. Agents get it through their own environment
    variables (`CODEX_CA_CERTIFICATE`, `NODE_EXTRA_CA_CERTS`).
- **Small code.** Functions with more than 3-4 arguments take a context struct. Delete dead code.
- **Pinned dependencies.** Every version is exact (`=x.y.z`) and at least two weeks old. Adding a
  dependency needs the user's approval.
- **Bounded runs.** Wrap long commands in `timeout` (default 300 s, never above 600 s). A real
  agent run through the proxy uses a harmless prompt, at most 4 calls per task, and `</dev/null`.
  On a timeout, stop and report; do not retry bigger.
- **Disk.** Debug builds only. Delete scratch tap files and merged worktrees promptly.

## Git and worktrees

The default branch is `main`.

From the repo root, create a worktree per lane with
`git worktree add .claude/worktrees/<lane> -b worktree-<lane> main`. The `.claude/worktrees/` folder
is ignored by `.gitignore`. Before the first edit, confirm you are on the branch `worktree-<lane>`
in that worktree.

Each commit message is one plain sentence, with no attribution lines. `CLAUDE.local.md` is never
committed.

Integrate by rebasing the lane onto `main`, running the gate, and opening a pull request. Merge
only after all CI checks pass. Nothing is pushed without the user's request.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:1105d646 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/core-concepts/sync-concepts.md for details and anti-patterns.

## Agent Context Profiles

The managed Beads block is task-tracking guidance, not permission to override repository, user, or orchestrator instructions.

- **Conservative (default)**: Use `bd` for task tracking. Do not run git commits, git pushes, or Dolt remote sync unless explicitly asked. At handoff, report changed files, validation, and suggested next commands.
- **Minimal**: Keep tool instruction files as pointers to `bd prime`; use the same conservative git policy unless active instructions say otherwise.
- **Team-maintainer**: Only when the repository explicitly opts in, agents may close beads, run quality gates, commit, and push as part of session close. A current "do not commit" or "do not push" instruction still wins.

## Session Completion

This protocol applies when ending a Beads implementation workflow. It is subordinate to explicit user, repository, and orchestrator instructions.

1. **File issues for remaining work** - Create beads for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Handle git/sync by active profile**:
   ```bash
   # Conservative/minimal/default: report status and proposed commands; wait for approval.
   git status

   # Team-maintainer opt-in only, unless current instructions forbid it:
   git pull --rebase
   git push
   git status
   ```
5. **Hand off** - Summarize changes, validation, issue status, and any blocked sync/commit/push step

**Critical rules:**
- Explicit user or orchestrator instructions override this Beads block.
- Do not commit or push without clear authority from the active profile or the current user request.
- If a required sync or push is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->

<!-- BEGIN BEADS CODEX SETUP: generated by bd setup codex -->
## Beads Issue Tracker

Use Beads (`bd`) for durable task tracking in repositories that include it. Use the `beads` skill at `.agents/skills/beads/SKILL.md` (project install) or `~/.agents/skills/beads/SKILL.md` (global install) for Beads workflow guidance, then use the `bd` CLI for issue operations.

### Quick Reference

```bash
bd ready                # Find available work
bd show <id>            # View issue details
bd update <id> --claim  # Claim work
bd close <id>           # Complete work
bd prime                # Refresh Beads context
```

### Rules

- Use `bd` for all task tracking; do not create markdown TODO lists.
- Run `bd prime` when Beads context is missing or stale. Codex 0.129.0+ can load Beads context automatically through native hooks; use `/hooks` to inspect or toggle them.
- Keep persistent project memory in Beads via `bd remember`; do not create ad hoc memory files.

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/core-concepts/sync-concepts.md for details and anti-patterns.
<!-- END BEADS CODEX SETUP -->
