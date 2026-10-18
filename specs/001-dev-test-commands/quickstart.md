# Quickstart: Validate Dev and Test Commands

Prerequisites: `task` installed, Rust toolchain installed, a clean checkout of the feature branch. Contract: [contracts/tasks.md](contracts/tasks.md). Wrap each run in `timeout 300`.

## 1. List the tasks (US5, FR-013)

```sh
task --list
```

Expect: `check`, `test:unit`, `test:integration`, `dev`, each with a purpose. A reader finds the unit test task from the README in under 1 minute (SC-002).

## 2. Full check passes (US1, SC-001)

```sh
task check; echo $?
```

Expect: exit 0.

## 3. Check fails on each kind of error (US1, SC-005)

Make one change at a time in a scratch worktree and revert it after each run.

| Change | Expect |
|--------|--------|
| Misformat a line in `src/lib.rs` | exit non-zero, `fmt` named |
| Add an unused variable | exit non-zero, `clippy` named |
| Make one test assert a wrong value | exit non-zero, `test` named |
| Make all three at once | exit non-zero, all three named in one run |

## 4. Unit and integration are separate (US2, SC-003)

```sh
task test:unit            # expect 0 tests from tests/proxy.rs
task test:unit -- sse::   # expect only sse tests
task test:integration     # expect only tests from tests/proxy.rs
```

Run `task test:integration` again with the network off. Expect the same result (offline).

## 5. Dev proxy (US3, SC-004)

```sh
DEV_TIMEOUT=20 task dev
```

Expect: stderr shows the listening address and `ca.pem` path under `target/dev/ca`. The run stops after 20 s. Then run `DEV_TIMEOUT=601 task dev` and expect non-zero exit with no proxy started. Confirm the tap file and `ca.pem` paths it prints are both under `target/dev/`, and that `git status` shows no new file outside `target/`. Delete `target/dev/` after the run.

## 6. Any directory (FR-009)

```sh
cd src && task test:unit
```

Expect: same result as from the repo root.

## 7. CI (US4, SC-006)

Open a pull request with a deliberately failing test. Expect CI to fail on the pull request at the test step. Fix it. Expect CI to pass. Confirm the workflow has fmt, clippy and test steps matching `scripts/check.sh`, and does not install or call Task.
