# Research: Dev and Test Commands

No `NEEDS CLARIFICATION` remained after `/speckit-clarify`. These are the plan-level decisions.

## Task names

- **Decision**: `check`, `test:unit`, `test:integration`, `dev`.
- **Rationale**: `check` and `dev` are fixed by the user. The `test:` namespace groups the two test tasks in `task --list`.
- **Alternatives**: `unit` and `integration` at top level. Rejected: no grouping.

## Unit versus integration split

- **Decision**: `test:unit` runs `cargo test --lib`. `test:integration` runs `cargo test --test proxy`. `task test:unit -- <module>::` passes the module filter through `{{.CLI_ARGS}}`.
- **Rationale**: Unit tests are the `#[cfg(test)]` modules in `src/` (9 files). Integration tests are `tests/proxy.rs`. The split matches the layout and the constitution.
- **Alternatives**: A custom cargo feature or `#[ignore]` marker. Rejected: needs code changes.
- **Risk**: A new file in `tests/` is not run by `test:integration` unless the task is updated. `check` runs plain `cargo test`, which covers everything, so the gate stays complete.

## Reporting every failing check

- **Decision**: `scripts/check.sh` runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, keeps going after a failure, prints the failed check names, and exits non-zero if any failed.
- **Rationale**: FR-004 and the edge case need all failures in one run. Task's default stops at the first failing command, and the body is over a few lines, so it goes in a script.
- **Alternatives**: Task `deps` run in parallel. Rejected: `cargo` locks the build directory, so parallel checks block each other, and output interleaves.

## Dev task

- **Decision**: `scripts/dev.sh` runs `cargo run -- --tap-file target/dev/tap.jsonl --ca-dir target/dev/ca` under `timeout`. Default 300 s. A `DEV_TIMEOUT` variable overrides it. The script refuses values above 600.
- **Rationale**: `--tap-file` is required by the binary. `target/` is git-ignored and the constitution asks for scratch files to be deleted, so `target/dev/` holds them and `cargo clean` removes them. A CA dir under `target/dev/` keeps development away from `~/.config/soze` and from every agent folder. The binary already prints the listening address and CA path on stderr, which meets FR-007.
- **Alternatives**: Default `~/.config/soze`. Rejected: shared state outside the repo. Running without `timeout`. Rejected: the constitution bounds runs.

## Working from any directory

- **Decision**: Rely on Task finding the nearest `Taskfile.yml` in parent directories and running tasks from its directory. Scripts use paths relative to the Taskfile directory.
- **Rationale**: Built-in behavior. Verified in the quickstart.

## CI

- **Decision**: One workflow, `.github/workflows/ci.yml`, triggered on `pull_request` and on `push` to `main`. Steps: `actions/checkout`, then `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, as separate steps so each failure shows by name. The Ubuntu runner already has Rust stable with rustfmt and clippy.
- **Rationale**: User direction: use normal Rust actions and best practice, no Task in CI. Integration tests are offline, so no secrets are needed. Separate steps give per-check results without a script.
- **Alternatives**: Install Task in CI and run `task check`. Rejected by the user. Third-party Rust toolchain or cache actions. Rejected: not needed, and each is a new dependency needing approval.
- **Drift risk**: CI and `scripts/check.sh` list the same three commands in two places. Accepted; the plan and reviews compare them (FR-012).

## Test framework

- **Decision**: Only `cargo test`. Scripts are verified by the quickstart trials, not by a shell test framework.
- **Rationale**: User direction (FR-014). A framework such as `bats` would be a new dependency.

## Docs

- **Decision**: README gets a "Tasks" list. The `AGENTS.md` Gate section names `task check` and drops the chained command.
- **Rationale**: FR-013. One definition of the gate.
