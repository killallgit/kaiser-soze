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
