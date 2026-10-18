---

description: "Task list for Dev and Test Commands"
---

# Tasks: Dev and Test Commands

**Input**: Design documents from `/specs/001-dev-test-commands/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/tasks.md, quickstart.md

**Tests**: No new Rust code is added, so no new `cargo test` test is written. The standard `cargo test` framework is the only test framework (FR-014). Each story ends with a verification task that runs the matching [quickstart.md](quickstart.md) section as its acceptance test. Run each verification first and see it fail (red) before the implementation tasks.

**Organization**: Tasks are grouped by user story. Every task that runs a long command wraps it in `timeout` (300 s default, never above 600 s). Nothing is committed or pushed until the user asks.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: User story the task belongs to (US1 to US5)

## Phase 1: Setup

- [X] T001 Create the lane worktree from the repo root: `git worktree add .claude/worktrees/dev-test-commands -b worktree-dev-test-commands main`. Work in it from here on and confirm `git branch --show-current` prints `worktree-dev-test-commands` before the first edit
- [X] T002 Move the uncommitted work into the worktree: copy `specs/`, `.specify/memory/constitution.md` (v1.2.0) and `AGENTS.md` (integration paragraph) into it, recreate the git-ignored `.specify/feature.json` pointing at `specs/001-dev-test-commands`, confirm `git status` there lists the same files, then remove `specs/` and restore the two edited files in the main checkout with `git checkout -- .specify/memory/constitution.md AGENTS.md`
- [X] T003 Create `Taskfile.yml` at the repo root with `version: '3'` and a default task that runs `task --list`

**Checkpoint**: lane worktree holds the spec and the amended constitution, and `task --list` runs from its root.

---

## Phase 2: Foundational

No blocking work beyond T001 to T003. All stories add tasks to the same `Taskfile.yml`, so their Taskfile edits run one after another, not in parallel.

---

## Phase 3: User Story 1 - Run every check with one command (Priority: P1) MVP

**Goal**: `task check` runs fmt, clippy and test, reports every failure, and exits non-zero on any failure.

**Independent Test**: [quickstart.md](quickstart.md) sections 2 and 3.

- [X] T004 [US1] Red step: run `task check` and confirm it fails because the task does not exist
- [X] T005 [US1] Create `scripts/check.sh` (POSIX shell, executable): run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`; keep going after a failure; print the name of each failed check (`fmt`, `clippy`, `test`); exit non-zero if any failed; run from the repo root so it works from any directory
- [X] T006 [US1] Add a `check` task to `Taskfile.yml` with a one-line `desc` that calls `scripts/check.sh`
- [X] T007 [US1] Verify: run quickstart section 2 (exit 0) and section 3 in a scratch worktree (misformat, unused variable, wrong assertion, all three at once). Each makes `task check` exit non-zero and name the failed checks. Revert each change and delete the scratch worktree

**Checkpoint**: MVP complete. `task check` is the one definition of green.

---

## Phase 4: User Story 2 - Run unit and integration tests separately (Priority: P2)

**Goal**: Two test tasks that never run each other's tests, with a module filter on the unit task.

**Independent Test**: [quickstart.md](quickstart.md) section 4.

- [X] T008 [US2] Red step: run `task test:unit` and `task test:integration` and confirm both fail because the tasks do not exist
- [X] T009 [US2] Add `test:unit` to `Taskfile.yml` with a `desc`: `cargo test --lib {{.CLI_ARGS}}`
- [X] T010 [US2] Add `test:integration` to `Taskfile.yml` with a `desc`: `cargo test --test proxy`
- [X] T011 [US2] Verify: run quickstart section 4. `test:unit` runs 0 tests from `tests/proxy.rs`, `task test:unit -- sse::` runs only `sse` tests, and `test:integration` runs only `tests/proxy.rs` tests; run `test:integration` again with the network off and expect the same result

**Checkpoint**: US1 and US2 both work.

---

## Phase 5: User Story 3 - Start the proxy for local development (Priority: P2)

**Goal**: `task dev` starts the proxy with scratch tap and CA dirs, bounded in time.

**Independent Test**: [quickstart.md](quickstart.md) section 5.

- [X] T012 [US3] Red step: run `task dev` and confirm it fails because the task does not exist
- [X] T013 [US3] Create `scripts/dev.sh` (POSIX shell, executable): read `DEV_TIMEOUT` (default 300); exit non-zero before starting anything if it is not a whole number from 1 to 600; create `target/dev/`; run `timeout "$DEV_TIMEOUT" cargo run -- --tap-file target/dev/tap.jsonl --ca-dir target/dev/ca`; treat the timeout stop (exit 124) and interrupt as success
- [X] T014 [US3] Add a `dev` task to `Taskfile.yml` with a `desc` that calls `scripts/dev.sh`
- [X] T015 [US3] Verify: run quickstart section 5 (`DEV_TIMEOUT=20 task dev` shows the listening address and `ca.pem` path under `target/dev/ca` and stops after 20 s; `DEV_TIMEOUT=601 task dev` exits non-zero and starts nothing). Check that the tap file and `ca.pem` paths it prints are both under `target/dev/` and that `git status` shows no new file outside `target/`, then delete `target/dev/`

**Checkpoint**: US1 to US3 work.

---

## Phase 6: User Story 4 - CI runs the same checks (Priority: P2)

**Goal**: CI runs fmt, clippy and test as separate steps with plain Rust steps. No Task in CI.

**Independent Test**: [quickstart.md](quickstart.md) section 7.

- [X] T016 [P] [US4] Create `.github/workflows/ci.yml`: trigger on `pull_request` and on `push` to `main`; one job on `ubuntu-latest` with `actions/checkout` pinned to a full commit SHA (look it up with `gh api` and confirm it is at least two weeks old), then three steps in this order: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`. No setup action, no cache action, no Task
- [X] T017 [US4] Verify (needs the user's go-ahead to push): open a pull request with a deliberately failing test, see CI fail at the test step, fix it, see CI pass. Confirm the three steps match `scripts/check.sh`

**Checkpoint**: CI result equals the local result.

---

## Phase 7: User Story 5 - Discover the commands (Priority: P3)

**Goal**: A new reader finds every task from the README, and `AGENTS.md` names `task check`.

**Independent Test**: [quickstart.md](quickstart.md) sections 1 and 6.

- [X] T018 [P] [US5] Add a "Tasks" section to `README.md` listing `task check`, `task test:unit` (with the `-- <module>::` filter), `task test:integration` and `task dev` (with `DEV_TIMEOUT`), one line each
- [X] T019 [P] [US5] In `AGENTS.md`, replace the Gate code block with `task check` and keep the line about testing a single module, now `task test:unit -- <module>::`
- [X] T020 [US5] Verify: run quickstart sections 1 and 6 (`task --list` shows all four tasks with purposes; `cd src && task test:unit` matches the root result; a reader finds the unit test task from the README in under 1 minute)

**Checkpoint**: all five stories work.

---

## Phase 8: Polish

- [X] T021 Run `task check` on the finished tree and confirm exit 0, and that `git diff main -- Cargo.toml Cargo.lock` shows no change (FR-014)
- [X] T022 Compare the three commands in `.github/workflows/ci.yml` with `scripts/check.sh` and confirm they match (FR-012)
- [X] T023 Delete scratch files: `target/dev/` and any scratch worktree from T007
- [X] T024 When the user asks: commit on the lane branch, rebase onto `main`, run `task check`, open a pull request, merge only after all CI checks pass, then remove the lane worktree

---

## Dependencies and Execution Order

- T001 to T003 first, in order. T004 to T007 (US1) next; US1 is the MVP.
- US2, US3 and US4 depend only on T003 (US2 and US3 also edit `Taskfile.yml`, so their Taskfile edits go one at a time).
- US5 writes the final docs, so run it after US1 to US3 so the listed tasks exist.
- Polish last.

```text
T001 -> T002 -> T003 -> US1 -> US2 -> US3 -> US5 -> Polish
T003 -> US4 (T016 any time after T003) -> T017 after US1
```

## Parallel Opportunities

- T016 (`.github/workflows/ci.yml`) can run alongside any Taskfile task.
- T018 (`README.md`) and T019 (`AGENTS.md`) run in parallel.
- T005 (`scripts/check.sh`) and T013 (`scripts/dev.sh`) touch different files and can be written in parallel, but each story's Taskfile edit still waits its turn.

## Implementation Strategy

1. MVP: T001 to T007. `task check` is usable at once.
2. Add US2 and US3, then US4 (CI), then US5 (docs).
3. Stop after each checkpoint and run the matching quickstart section.
4. Open the pull request last, and merge only after CI passes.

## Notes

- Total: 24 tasks. Setup 3, US1 4, US2 4, US3 4, US4 2, US5 3, Polish 4.
- Wording of task descriptions, scripts and docs avoids message content and logs; no test asserts on message strings.

---

## Phase 9: Convergence

- [X] T025 Prove CI goes red: on a throwaway branch push one deliberately failing test, confirm the `ci` job fails at the `cargo test` step, then delete the branch, per US4/AC2 and SC-006 (partial)
- [X] T026 Prove the integration tests pass offline: run `task test:integration` with the network switched off and confirm the same 5 tests pass, per the "pass offline" edge case (partial)
- [X] T027 Prove `task dev` stops cleanly on interrupt: start `task dev`, send SIGINT, confirm exit 0 and no leftover `soze` process, then delete `target/dev/`, per US3/AC2 (partial)
- [ ] T028 Time the README discovery: from a cold read of `README.md`, find and run `task test:unit` and confirm it takes under 1 minute, per SC-002 (partial)
