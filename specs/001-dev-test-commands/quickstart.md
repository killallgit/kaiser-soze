# Quickstart: Validate Dev and Test Commands

Prerequisites: `task` installed, Rust toolchain installed, a clean checkout of the feature branch. Contract: [contracts/tasks.md](contracts/tasks.md). Bound automated checks with `timeout 300`; the shared dev service runs until explicitly stopped.

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
task dev
```

Expect: stderr shows the listening address on port 48731 and the CA path `~/.config/soze/ca.pem`. Tap files are under `target/dev/`. Agents and consumer apps share this service; it stays up until explicitly stopped with Ctrl-C. For an isolated automated smoke test, choose an unused port and wrap that test invocation in an external timeout. Do not stop the shared service or delete its tap files as test cleanup.

## 6. Any directory (FR-009)

```sh
cd src && task test:unit
```

Expect: same result as from the repo root.

## 7. CI (US4, SC-006)

Open a pull request with a deliberately failing test. Expect CI to fail on the pull request at the test step. Fix it. Expect CI to pass. Confirm the workflow has fmt, clippy and test steps matching `scripts/check.sh`, and does not install or call Task.
