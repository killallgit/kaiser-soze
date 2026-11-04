# kaiser-soze

kaiser-soze is one Rust crate with one binary, `soze`: a local proxy that sits between a coding
agent and its model API, forwards every byte as it arrives, and taps a copy of the stream into
events. Each tool is an `Adapter`. Codex and Claude Code are implemented.

Before inspecting implementation or planning changes, read
[docs/objective.md](docs/objective.md); it defines the whole scope. Read
[docs/common-format.md](docs/common-format.md) when working with the event shape.

## Gate

After any repository change, run:

```
timeout 300 task check
```

It runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` and the
wrapper and service shell tests, and names every failed check. Inspect the complete result, not
only a truncated tail. To test a single module, run `task test:unit -- <module>::`. A latent failure
the gate exposes gets fixed, even if unrelated.

## Rules

- **Never in the way.** Forwarding never waits on the tap. A slow or failing parser drops and
  counts; it never delays, alters or breaks the bytes the agent receives.
- **Tests first.** Write the failing test, then the code. Fixtures are synthetic, never captured
  real traffic. Each test follows Assemble, Act, Assert, with a comment describing the assertion.
  Unit and integration tests stay separate.
- **No log or error text in tests.** Assert on types, values and events, never on message strings.
  A failure is checked by its concrete error type; a test that cannot do that signals a refactor.
- **Data safety.**
  - Message content is stored in tap output but never printed in a terminal, chat log, test
    output or commit. Report counts and ids only.
  - Credential headers are redacted before anything is written.
  - Never open credential stores (`~/.codex/auth.json`, Cursor `ItemTable`, keychains).
  - Never write under agent folders (`~/.claude`, `~/.codex`, `~/.cursor`).
  - Never install the CA into a trust store. Agents get it through their own environment
    variables (`CODEX_CA_CERTIFICATE`, `NODE_EXTRA_CA_CERTS`).
- **Small code.** Functions with more than 3-4 arguments take a context struct. Delete dead code;
  a function only a test calls is dead, so delete both. Encapsulate state: global mutable state
  needs the user's confirmation. A "utility" module signals an undefined domain.
- **Verify agent output.** Validate and test it as if a new hire who has not read these rules wrote
  it.
- **Fix stale docs** when you find them during work.
- **Pinned dependencies.** Every version is exact (`=x.y.z`) and at least two weeks old. Prefer
  libraries that are actively maintained and have verifiable production users.
- **Bounded runs.** Wrap long commands in `timeout` (default 300 s, never above 600 s). A real
  agent run through the proxy uses a harmless prompt, at most 4 calls per task, and `</dev/null`.
  On a timeout, stop and report; do not retry bigger.
- **Disk.** Debug builds only. Delete scratch tap files and merged worktrees promptly.
- **Name what an id points at.** Follow every bead, PR or issue id (`#22`, `kaiser-soze-7ug`) with
  a few-word description in parentheses, for example `kaiser-soze-7ug (graph edges)`. The reader
  often does not remember the id, so it should say what the item is at a glance.

## Git and worktrees

The default branch is `main`.

Before the first edit, create a lane worktree from the repo root with
`git worktree add .claude/worktrees/<lane> -b worktree-<lane> main`, enter it, and confirm the
branch is `worktree-<lane>`. This setup is required even when commits or pushes are not authorized;
it does not authorize either. `.claude/worktrees/` is ignored by `.gitignore`.

Each commit message is one plain sentence, with no attribution lines. `CLAUDE.local.md` is never
committed.

Integrate by rebasing the lane onto `main`, running the gate, and opening a pull request. Merge
only after all CI checks pass. Nothing is pushed without the user's request.

To get a pull request reviewed, use the `independent-review` skill
(`killall-skills:independent-review`); it holds the review rules.

## Beads

For task tracking, persistent project memory, or implementation handoff, use the `beads` skill and
run `bd prime` first.
