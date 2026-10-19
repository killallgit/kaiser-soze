# Research

## Default port
- **Decision**: 48731.
- **Rationale**: five digits, because most dev tools sit on four-digit ports (3000, 5173, 8080, 8000); just under 49152, where macOS starts handing out outgoing-connection ports; absent from `/etc/services`; not listening on this machine; not a conventional number.
- **Known limit**: Linux's default outgoing range starts at 32768, so 48731 can in principle be taken by a client socket when soze starts. soze then fails to bind and says so; the wrappers see no listener and run the agent untouched.
- **Alternatives**: 8765 and similar four-digit ports (common in tools and tutorials); a port at or above 49152 (macOS ephemeral, collides more often).

## Detecting a live proxy
- **Decision**: `nc -z -w 1 127.0.0.1 $SOZE_PORT`, plus a readable CA file; no `nc` means not ready.
- **Rationale**: works in sh, zsh and bash on macOS and Linux; a closed local port refuses at once, so a dead soze adds no delay; failing closed to "not ready" keeps the agent working.
- **Alternatives**: `/dev/tcp` (bash only); a pid or lock file (goes stale after a crash and reports ready when soze is dead); `curl` to the proxy (a heavier dependency).

## Scoping the variables to one command
- **Decision**: `env VAR=... agent "$@"`.
- **Rationale**: nothing leaks into the shell, and `env` runs the binary on `PATH`, so the function does not recurse.
- **Alternatives**: a subshell with `export` (more code, same effect); `VAR=... command agent` (POSIX leaves variable persistence around special builtins unspecified, and shells differ).

## Where the install writes
- **Decision**: one file at `~/.config/soze/wrappers.sh` and one marked block in `~/.zshrc` or `~/.bashrc`, picked from `$SHELL`.
- **Rationale**: the block is one `.` line, so uninstall is deleting a marker range; the file keeps the logic out of the start-up file and can be replaced on upgrade.
- **Alternatives**: inline functions in the start-up file (hard to update and remove); symlinks on `PATH` (shadow the real binary and break when the agent re-execs itself).

## Dev and the CA
- **Decision**: `task dev` keeps its CA under `target/dev/ca`; to capture with the wrappers during dev, set `SOZE_CA_DIR=$PWD/target/dev/ca`.
- **Rationale**: keeps dev isolated from the real CA dir, as spec 001 intends.
- **Alternatives**: dev uses `~/.config/soze` (mixes scratch and real state).
