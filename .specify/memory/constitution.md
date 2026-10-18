# kaiser-soze Constitution

## Core Principles

### I. Never in the Way (NON-NEGOTIABLE)

The proxy MUST forward every byte as it arrives. Forwarding MUST NOT wait on the tap. A slow or
failing parser MUST drop the tap data and count the drop. It MUST NOT delay, alter or break the
bytes the agent receives.

Rationale: the proxy sits in front of a working agent. A tap failure may cost tap data only.

### II. Adapter per Tool

Each coding agent MUST be supported by one `Adapter` (`src/adapter.rs`). Support for a new tool
MUST NOT require a change to the core. Events MUST follow `docs/common-format.md`, which keeps
only what the model sees and says.

Rationale: every agent talks to a model API over HTTPS, so a small per-tool adapter keeps the
design future-proof. `docs/objective.md` is the whole scope.

### III. Tests First

The failing test MUST be written before the code. Fixtures MUST be synthetic, never captured
real traffic. Tests MUST assert on types, values and events, never on log or error message
strings. To check a failure, a test MUST assert the concrete error type. A test that cannot do
this signals a design smell and MUST prompt a refactor.

- Each test MUST follow Assemble, Act, Assert, and its comment MUST describe the assertion.
- Tests MUST use dependency injection and stay functional.
- Unit tests and integration tests MUST be separate. Integration tests MUST run on their own.

Rationale: synthetic fixtures keep private traffic out of the repo. Message-string assertions
break on wording changes that alter no behavior.

### IV. Data Safety (NON-NEGOTIABLE)

- Message content MAY be stored in tap output. It MUST NOT be printed in a terminal, chat log,
  test output or commit. Reports MUST give counts and ids only.
- Credential headers MUST be redacted before anything is written.
- Credential stores (`~/.codex/auth.json`, Cursor `ItemTable`, keychains) MUST NOT be opened.
- Nothing MUST be written under agent folders (`~/.claude`, `~/.codex`, `~/.cursor`).
- The CA MUST NOT be installed into a trust store. Agents MUST get it through their own
  environment variables (`CODEX_CA_CERTIFICATE`, `NODE_EXTRA_CA_CERTS`).

Rationale: the tap sees prompts and credentials. A leak is not reversible.

### V. Small Code

Functions with more than 3-4 arguments MUST take a context struct. Dead code MUST be deleted.
The output transport and store are undecided; code MUST NOT assume one until the user decides.

- Code MUST depend on interfaces (traits). New behavior SHOULD extend, not modify, existing code.
- Global mutable state MUST NOT be used without the user's confirmation and a written reason.
  State MUST be encapsulated.
- Configuration and context MUST be immutable structs, not long argument lists.
- Each function MUST do one thing and have a descriptive name. Pure functions and injected
  dependencies SHOULD be preferred.
- A "utility" module signals an undefined domain. Code MUST NOT add one without a stated reason.

Rationale: small code is easy to review, and the open transport question stays open.

### VI. Simplify and Delete

Code that can be simplified MUST be simplified. Code that can be deleted MUST be deleted, because
git keeps history. A function called only by a test is dead: delete the function and the test.
Code MUST NOT be written to satisfy an imagined strict review.

### VII. Self-Documenting Code

Names, file layout and tests MUST tell the story, so a first-time reader understands the code
without help. A comment MUST NOT repeat what the code shows. Documentation MUST NOT go into
detail that goes stale, and stale documentation found during work MUST be fixed.

### VIII. Verify Everything

Information from any agent MUST NOT be taken at face value. Agent output MUST be validated,
verified and tested, as if a junior developer with no knowledge of project rules wrote it.

## Data Safety and Dependencies

- Every dependency version MUST be exact (`=x.y.z`) and at least two weeks old.
- Adding a dependency MUST have the user's approval.
- Long commands MUST run under `timeout` (default 300 s, never above 600 s). A real agent run
  through the proxy MUST use a harmless prompt, at most 4 calls per task, and `</dev/null`. On a
  timeout, work MUST stop and report, not retry bigger.
- Builds MUST be debug builds only. Scratch tap files and merged worktrees MUST be deleted
  promptly.

## Development Workflow

- The gate MUST pass before integration:
  `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`.
- Lint and typecheck MUST be green before any push and in CI.
- A change that exposes an unrelated latent failure in tests, lint or typecheck MUST stop and fix
  it. The exception is urgent work: then a parallel agent SHOULD fix it.
- Each lane MUST work in its own worktree on branch `worktree-<lane>`, created from `main` under
  `.claude/worktrees/`.
- Each commit message MUST be one plain sentence with no attribution lines. `CLAUDE.local.md`
  MUST NOT be committed.
- Integration MUST rebase the lane onto `main`, run the gate, and fast-forward local `main`.
  Nothing MUST be pushed without the user's request.

## Governance

This constitution supersedes other practices. `AGENTS.md` is the runtime guidance and MUST stay
consistent with it. An amendment MUST be a documented change to this file with a version bump,
approved by the user. Versioning follows semantic rules: MAJOR for removed or redefined
principles, MINOR for new or materially expanded principles, PATCH for clarifications. Every
plan and review MUST check compliance. A violation MUST be fixed or justified in writing.

**Version**: 1.1.0 | **Ratified**: 2026-10-17 | **Last Amended**: 2026-10-17
