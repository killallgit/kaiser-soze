# Tasks: Agent wrappers

## Phase 1: Setup

- [ ] T001 Create `tests/wrappers/run.sh` (runs every `tests/wrappers/test_*.sh` in a throwaway `HOME`, prints failed case names, exits non-zero on any failure) and `tests/wrappers/lib.sh` (helpers: make a stub agent that writes its `HTTPS_PROXY`, `NODE_EXTRA_CA_CERTS`, `CODEX_CA_CERTIFICATE` and arguments to a file and exits with a given status; assert-equal that prints only the case name). Add `test:wrappers` to `Taskfile.yml` and a `run wrappers sh tests/wrappers/run.sh` line to `scripts/check.sh`.

## Phase 2: US1 and US2, the wrappers (P1)

Tests first. Tests override the probe with `_soze_probe() { return 0; }` after sourcing, per [plan.md](plan.md).

- [ ] T002 [US1] Add failing `tests/wrappers/test_ready.sh`: with probe true and a CA file present, each of `claude`, `codex`, `agent` runs its stub with exactly the variables in the `data-model.md` table (`HTTPS_PROXY=http://127.0.0.1:48731`, `NODE_EXTRA_CA_CERTS=$SOZE_CA_DIR/ca.pem`, `CODEX_CA_CERTIFICATE=$SOZE_CA_DIR/ca.pem`); `SOZE_PORT` and `SOZE_CA_DIR` override the defaults; arguments (including one with a space) and the exit status pass through; stdin reaches the agent; the variables are not set in the shell afterwards.
- [ ] T003 [US2] Add failing `tests/wrappers/test_fail_open.sh`, one case each, asserting the stub saw no proxy variable and ran with the same arguments and exit status: real `nc` against a closed port (default probe, port unused); probe function missing; `nc` absent from `PATH`; CA file missing; CA file unreadable; `HTTPS_PROXY` set; `https_proxy` set; `SOZE_OFF=1`; wrapper helper `_soze_ready` undefined. Also assert the wrapper prints nothing to stdout or stderr in every case.
- [ ] T004 [US1] Write `scripts/soze-wrappers.sh` per [contracts/wrappers.md](contracts/wrappers.md) and [plan.md](plan.md): POSIX `sh` syntax, `_soze_probe` (`command -v nc` then `nc -z -w 1 127.0.0.1 "${SOZE_PORT:-48731}"`), `_soze_ready` (true only when "`SOZE_OFF` empty, `HTTPS_PROXY` and `https_proxy` unset, probe succeeds, `$SOZE_CA_DIR/ca.pem` readable"), and `claude`, `codex`, `agent` that run `env VAR=... <agent> "$@"` when ready and `command <agent> "$@"` otherwise. No output, no `export`. T002 and T003 pass.
- [ ] T005 [US2] Make a wrapper survive its own helper being absent: each function calls `_soze_ready` as `if _soze_ready 2>/dev/null; then ... else command <agent> "$@"; fi`, so an undefined helper takes the else branch. Confirm the last case of T003 passes.

## Phase 3: US3, install and remove (P2)

- [ ] T006 [US3] Add failing `tests/wrappers/test_install.sh`, each case in a throwaway `HOME` with a `$SHELL` of zsh or bash: install writes `~/.config/soze/wrappers.sh` and appends the exact block from `contracts/wrappers.md` to `~/.zshrc` or `~/.bashrc`; second install leaves both files byte-identical; a backup of the start-up file exists after a changing install; `--dry-run` writes no file and prints only paths and the block; uninstall restores the start-up file byte-for-byte (including a file with no final newline, where only the added final newline may differ), removes the wrappers file, and is a no-op when nothing is installed; a file with a block but no wrappers file is repaired by install; an unknown argument exits non-zero; nothing is created under `~/.claude`, `~/.codex`, `~/.cursor`.
- [ ] T007 [US3] Write `scripts/wrappers.sh` (`install` and `uninstall`, `--dry-run`): pick the start-up file from `$SHELL` (zsh gives `~/.zshrc`, bash gives `~/.bashrc`, other shells exit non-zero with no change), copy `scripts/soze-wrappers.sh` from the script's own directory, add or remove the marked block, back up to `<file>.soze-backup` before any change. T006 passes.
- [ ] T008 [P] [US3] Add `wrappers:install` and `wrappers:uninstall` tasks to `Taskfile.yml` that run `scripts/wrappers.sh install|uninstall {{.CLI_ARGS}}`.

## Phase 4: Polish

- [ ] T009 Pass `--port "${SOZE_PORT:-48731}"` to `soze` in `scripts/dev.sh` so `task dev` listens where the wrappers look.
- [ ] T010 [P] Document in `README.md`: install and remove, the three settings, `SOZE_OFF=1` and `command claude` to bypass, `SOZE_CA_DIR=$PWD/target/dev/ca` for `task dev`, that wrappers fail open, and the known limit that a session started while soze runs loses its connection if soze dies.
- [ ] T011 Run `task check`.
- [ ] T012 In a throwaway `HOME` (not the real one), install, run a stub through the wrapper with `task dev` running, then uninstall; then real use once by hand: `task wrappers:install`, new shell, `task dev`, `SOZE_CA_DIR=$PWD/target/dev/ca claude` — report counts from `target/dev/tap.jsonl` only, never content — then stop `task dev`, run `claude` again and confirm it starts normally. T012 needs a person for the interactive `claude` step.
- [ ] T013 [P] Mark roadmap item 6's shell-function part done in `docs/roadmap.md`.

## Dependencies

T001 → T002, T003 → T004 → T005. T006 needs T001 and T004 (it copies the wrappers file). T007 → T008. T009, T010 and T013 are independent once T004 and T007 exist. T011 → T012.

## Parallel

T002 and T003 (different files). T008, T010, T013 (different files).

## Strategy

MVP is T001–T005: wrappers that capture when soze runs and are inert when it does not. Install and remove (T006–T008) follow. The fail-open tests (T003) gate everything else.
