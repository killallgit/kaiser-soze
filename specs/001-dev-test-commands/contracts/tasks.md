# Contract: Task Interface

The user-facing interface of this feature is the set of tasks. Callers: contributors, coding agents, CI.

| Task | Arguments | Behavior | Exit |
|------|-----------|----------|------|
| `task check` | none | Runs `fmt`, `clippy` (warnings are errors) and `test`. Runs all three even after a failure. Prints the names of failed checks. | 0 if all pass, else non-zero |
| `task test:unit` | optional `-- <filter>`, e.g. `-- sse::` | Runs only the unit tests in `src/`. The filter limits the run to matching tests. | cargo's test status |
| `task test:integration` | none | Runs only the tests in `tests/`. Needs no network or credentials. | cargo's test status |
| `task dev` | env `SOZE_PORT` (default 48731), `LIVE=0` to disable live summaries; flags after `--`, including `--stream`, `--log-level`, `--log-format` and `--log-file` | Builds and starts the shared debug proxy through `cargo run`, with tap files under `target/dev/` and CA under `~/.config/soze/`. Resets metadata-only `target/dev/debug.jsonl` on startup (16 MiB per-run cap). Logs the listening port and CA path. Runs until explicitly stopped; no automatic timeout. | Cargo/proxy exit status, including startup failure or interruption |
| `task dev:stream` | same environment as `dev`; optional flags after `--` | Runs the same shared dev script with `--stream --log-format human`. | Same as `dev` |
| `task dev:stream:json` | same environment as `dev`; optional flags after `--` | Runs the same shared dev script with `--stream --log-format json`. | Same as `dev` |

## Guarantees

- Every task works from any directory inside the repo.
- No task installs a CA into a trust store.
- No task writes under `~/.claude`, `~/.codex` or `~/.cursor`.
- `task --list` shows each task with its one-line purpose.
- CI runs the same three checks as `task check` with plain `cargo` steps. CI does not call Task.
