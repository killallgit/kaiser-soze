# Contract: Task Interface

The user-facing interface of this feature is the set of tasks. Callers: contributors, coding agents, CI.

| Task | Arguments | Behavior | Exit |
|------|-----------|----------|------|
| `task check` | none | Runs `fmt`, `clippy` (warnings are errors) and `test`. Runs all three even after a failure. Prints the names of failed checks. | 0 if all pass, else non-zero |
| `task test:unit` | optional `-- <filter>`, e.g. `-- sse::` | Runs only the unit tests in `src/`. The filter limits the run to matching tests. | cargo's test status |
| `task test:integration` | none | Runs only the tests in `tests/`. Needs no network or credentials. | cargo's test status |
| `task dev` | env `DEV_TIMEOUT` seconds, default 300, maximum 600 | Starts `soze` with the tap file in `target/dev/tap.jsonl` and the CA dir in `target/dev/ca`. The binary prints the listening address and CA path. Stops after the timeout or on interrupt. | 0 on interrupt or timeout stop, non-zero if the proxy fails to start or `DEV_TIMEOUT` is above 600 |

## Guarantees

- Every task works from any directory inside the repo.
- No task installs a CA into a trust store.
- No task writes under `~/.claude`, `~/.codex` or `~/.cursor`.
- `task --list` shows each task with its one-line purpose.
- CI runs the same three checks as `task check` with plain `cargo` steps. CI does not call Task.
