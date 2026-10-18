# Implementation Plan: Dev and Test Commands

**Branch**: `001-dev-test-commands` | **Date**: 2026-10-18 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/001-dev-test-commands/spec.md`

## Summary

Add a root `Taskfile.yml` with four tasks: `check`, `test:unit`, `test:integration`, `dev`. Logic longer than a few lines lives in `scripts/`. Add one GitHub Actions workflow that runs the same three checks with plain `cargo` steps, without Task. Update the README and `AGENTS.md` to name the tasks. No Rust code changes and no new Rust dependency. Scope stays inside [docs/objective.md](../../docs/objective.md): `task dev` writes the tap to a local file only, and assumes no transport or store.

## Technical Context

**Language/Version**: Rust 2021 crate (existing); POSIX shell for `scripts/`; Taskfile v3 (`task` 3.53.1 installed locally; contributors only, not CI)

**Primary Dependencies**: `task` binary (contributors install it); no new Rust dependency and no CI setup action

**Storage**: N/A. `task dev` writes scratch files under `target/dev/` (git-ignored)

**Testing**: standard `cargo test` only; no other test framework. Unit tests are the `#[cfg(test)]` modules in `src/` (`cargo test --lib`). Integration tests are `tests/proxy.rs` (`cargo test --test proxy`)

**Target Platform**: macOS (local), Linux (GitHub-hosted runner)

**Project Type**: CLI crate (single binary `soze`)

**Performance Goals**: N/A

**Constraints**: `task dev` is bounded (default 300 s, never above 600 s); debug builds only; tasks work from any directory in the repo; integration tests pass offline

**Scale/Scope**: 4 tasks, 2 scripts, 1 workflow, 2 doc edits

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Note |
|-----------|--------|------|
| I. Never in the Way | Pass | No proxy code changes |
| II. Adapter per Tool | Pass | No core or adapter changes |
| III. Tests First | Pass with note | No Rust code is added, so no new `cargo test` test is needed. Scripts get no other test framework (FR-014). The deliberate-failure trials in [quickstart.md](quickstart.md) are run against the scripts as their acceptance tests (SC-005, SC-006) |
| IV. Data Safety | Pass | `task dev` uses a CA dir under `target/dev/`, never installs the CA, never writes under agent folders |
| V. Small Code | Pass | Scripts are short, one job each; no utility module |
| VI. Simplify and Delete | Pass | The chained gate in `AGENTS.md` is replaced by `task check` |
| VII. Self-Documenting Code | Pass | Task names describe the job; no repeated docs |
| VIII. Verify Everything | Pass | Quickstart trials verify behavior |
| Dependencies | Pass | No Rust dependency and no third-party CI action. CI uses only the runner's built-in Rust toolchain and first-party `actions/checkout` |
| Workflow | Pass | `task check` is the gate (constitution v1.2.0); work happens in a lane worktree and merges by pull request after CI passes |

Post-design re-check: pass, unchanged.

## Project Structure

### Documentation (this feature)

```text
specs/001-dev-test-commands/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── tasks.md
└── tasks.md             # /speckit-tasks output, not created here
```

### Source Code (repository root)

```text
Taskfile.yml                  # check, test:unit, test:integration, dev
scripts/
├── check.sh                  # runs fmt, clippy, test; reports every failure
└── dev.sh                    # bounded proxy start with scratch tap and CA dirs
.github/workflows/ci.yml      # checkout, then cargo fmt --check, clippy -D warnings, test
README.md                     # lists the tasks (edit)
AGENTS.md                     # Gate section names `task check` (edit)
```

**Structure Decision**: Existing single-crate layout is unchanged. New files sit at the repo root and under `scripts/` and `.github/workflows/`.

## Complexity Tracking

No constitution violations to justify.
