# Plan: Agent wrappers

All shell, no Rust and no new dependency. Spec: [spec.md](spec.md). Decisions: [research.md](research.md). Names and formats: [data-model.md](data-model.md), [contracts/wrappers.md](contracts/wrappers.md). Validation: [quickstart.md](quickstart.md).

## Files

- `scripts/soze-wrappers.sh`: the file install copies to `~/.config/soze/wrappers.sh`. POSIX `sh` syntax, sourced by zsh and bash. Defines `claude`, `codex` and `agent`.
- `scripts/wrappers.sh`: `install` and `uninstall`, both with `--dry-run`.
- `Taskfile.yml`: `wrappers:install`, `wrappers:uninstall` (arguments after `--` go to the script), `test:wrappers`.
- `tests/wrappers/`: shell tests with stub agents, run by `tests/wrappers/run.sh`. `scripts/check.sh` runs them, so `task check` and CI cover them.
- `scripts/dev.sh`: pass `--port "${SOZE_PORT:-29817}"` so `task dev` listens where the wrappers look.
- `README.md`: install, remove, bypass, the dev CA note, and the known limit (soze dying mid-session).

## Wrapper shape

Each wrapper asks one function, `_soze_ready`, and then runs the agent one of two ways. Ready: `env HTTPS_PROXY=... <CA var>=... <agent> "$@"`. Not ready: `command <agent> "$@"`. `_soze_ready` is true only if `SOZE_OFF` is unset, no `HTTPS_PROXY`/`https_proxy` is set, `nc` exists, `nc -z -w 1 127.0.0.1 $SOZE_PORT` succeeds and `$SOZE_CA_DIR/ca.pem` is readable. Every other case is false. `env` runs the real binary found on `PATH`, so the wrapper never calls itself.

## Tests first

`tests/wrappers/` cases, each in a throwaway `HOME` with stub agents that record their environment and arguments:
- ready: variables set per agent, arguments and exit status pass through, nothing exported into the shell
- not ready, one case each: nothing listening (real `nc` against a closed port), probe unavailable, CA missing, `HTTPS_PROXY` set, `SOZE_OFF=1`: the stub sees no proxy variable
- install twice equals install once; uninstall restores the start-up file; uninstall with nothing installed changes nothing; `--dry-run` writes nothing; backup written; nothing under `~/.claude`, `~/.codex`, `~/.cursor`

The probe is a function (`_soze_probe`) so tests override it instead of opening a listener.

## Constitution check

No content printed, no new dependency, no credential store opened, nothing written under agent folders, no CA installed into a trust store, forwarding untouched. Tests use stubs and assert values. Pass before and after design.
