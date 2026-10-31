# Feature Specification: Always-on macOS service for development

**Status**: Draft. One caution is Open (see Open); the rest were settled in a design grilling on 2026-10-30 and after review.
**Input**: Epic `kaiser-soze-4dn` (run soze as an always-on local macOS service for development) and its children `kaiser-soze-4dn.1` (this spec), `kaiser-soze-4dn.3` (service shell tests), `kaiser-soze-65w` (install as a launchd user agent from a local debug build), `kaiser-soze-4dn.4` (restart, reload, status, logs and uninstall commands) and `kaiser-soze-4dn.5` (wire the tasks, `task check` and the README). The roadmap items are service and install (item 6) and the dev half of item 8. Configuration reload ([008](../008-config-reload/spec.md)) is what lets `launchctl kill HUP` change the live settings.

## Problem

`soze` must run all the time while it is developed, so it has to start at login and be updated easily from the current checkout. Today it runs only as `task dev` in a terminal, on one port, and stops with the terminal.

What exists, which the service has to fit:

- `soze` takes `--port`, `--tap-file`, `--shape-file` and `--log-file` as flags, and `log_format`, `log_level`, `live` and `stream` from the configuration file.
- `SIGHUP` re-reads the file and applies the live keys (`log_level`, `live`, `stream`); `log_format` and the paths need a restart (008).
- Tap and shape files rotate into numbered closed segments and are bounded by the retention table.
- `tests/wrappers/` holds portable shell tests with a throwaway home; the service tests follow it.

The scope is a launchd user agent installed from a local debug build. The CI release flow, `soze update`, Linux and a menu bar app are deferred. CI is unchanged: no macOS job, and the service tests are local shell tests run by `task check` only.

## User Story 1 (P1): Install from the current checkout

A person runs `task service:install` and `soze` is running, and starts again at every login, with the build from their checkout.

**Independent test**: with a throwaway home, a fake `launchctl`, a fake `git` and a fake binary, run `install` twice. After the first run the files exist as the contracts say and the agent is bootstrapped. After the second run there is still one agent, it runs the new binary, and the previous `soze.log` is `soze.log.1`.

**Acceptance**
1. `~/.local/share/soze/` holds `bin/soze`, `bin/install.json`, `logs/soze.log` (and `logs/soze.log.1`), `tap.jsonl`, `shapes.jsonl` and the debug file `debug.jsonl`. `~/.config/soze/` keeps `config.toml` and `ca.pem`. The plist is in `~/Library/LaunchAgents/`.
2. The service owns port 48731. `task dev` moves to port 48732 (`kaiser-soze-4dn.2`, move `task dev` to port 48732).
3. The agent starts at login and is restarted if it exits (`RunAtLoad` and `KeepAlive`). There is no socket activation.
4. The plist passes only the port and the output paths as flags. Log format, log level and `live` come from `config.toml`, so a `SIGHUP` reload can change them.
5. `config.toml` is created with `log_format = "human"` and `live = true` if it is absent, and is never overwritten.
6. The install logic is `scripts/service.sh`. There is no Rust `soze install` yet.
7. `service:install` builds a debug binary, copies it, writes the plist, rotates the log by one generation and restarts the agent. It is also how the service is updated: a second run is the update path.
8. `install.json` records the git sha, the branch, a dirty flag and the time. A dirty tree is not refused.
9. Nothing is written outside the home directory the script runs under.

## User Story 2 (P1): Operate the service

**Independent test**: against the fake `launchctl`, run `restart`, `reload`, `status` and `logs` with the agent loaded and not loaded. Each calls `launchctl` as the contracts say, `status` prints the lines in the contract, and `logs` runs `tail` on the service log and on no other file.

**Acceptance**
1. The tasks are `service:install`, `service:restart`, `service:reload`, `service:logs`, `service:status` and `service:uninstall`. There is no `service:update`.
2. `restart` is abrupt: `launchctl kickstart -k`. In-flight streams are cut, and a new `run_id` marks the restart.
3. `reload` sends `SIGHUP` to the agent (`launchctl kill HUP`), so the live keys in `config.toml` are re-read without a restart.
4. `status` shows the launchd state and pid, and, from `install.json`, whether the build is a release or a local build, the sha, the branch and the dirty flag.
5. `service:logs` tails `soze.log` and never the tap, shape or debug file: message content must not reach a terminal.

## User Story 3 (P1): Uninstall without losing captures

**Independent test**: install, then `uninstall`: the plist, `bin/soze` and `bin/install.json` are gone, and the captures, `logs/` and `~/.config/soze` are untouched. Then `uninstall --purge`: the paths under the data directory are listed on stdout, then the data directory is deleted.

**Acceptance**
1. `uninstall` boots the agent out, removes the plist, `bin/soze` and `bin/install.json`, and keeps the captures, the logs and the config.
2. `uninstall --purge` deletes the data directory after listing it. The list is paths only, never contents.

## User Story 4 (P2): Service tests that run locally

**Independent test**: `scripts/check.sh` runs `tests/service/run.sh`; every test fails against a missing `scripts/service.sh` and passes against the real one.

**Acceptance**
1. `tests/service/` holds shell tests (`run.sh`, `lib.sh`, `test_*.sh`) with a fake `launchctl`, a fake `soze` and a throwaway `HOME`, like `tests/wrappers/`.
2. They run from `scripts/check.sh`, and so from `task check`. They are not in `.github/workflows/ci.yml`.
3. The shell is portable (no `stat -f`), because the tests also run on Linux dev machines.
4. A test asserts that the `logs` command never opens the tap file, and that no file outside the throwaway home is written.

## Design

- **`scripts/service.sh`** is one POSIX shell script with the commands `install <binary>`, `restart`, `reload`, `status`, `logs` and `uninstall [--purge]`. The binary is built by the task, not by the script; the script takes a path. It calls `launchctl`, `git` and `tail` through `PATH`, which is the seam the tests use.
- **Install is idempotent.** The binary is copied beside the target and renamed over it, so the new file has a new inode (see the install sequence). The plist is written beside the target and compared with the current one, so an unchanged plist while the agent is loaded is a `kickstart -k`, and a changed plist is a `bootout` then a `bootstrap`.
- **Taskfile.** `service:install` builds the debug binary and calls the script with its path. The other five tasks are thin wrappers. The README documents the layout, the ports, how to update the service and that the service tests are local only.
- **`task dev` and the service share a user config.** `~/.config/soze/config.toml` is also the user config that `task dev` reads, so `task dev` picks up `log_format = "human"` and `live = true` too. Once `config.toml` holds `live = true`, `LIVE=0 task dev` no longer turns live lines off: with no flag the file's value applies, and `dev.sh` passes `--live` unless `LIVE=0`.

## Contracts

The paths, the script interface, the file formats and the test doubles below are the shared contract for the script, its tests and the Taskfile. They are quoted by the later beads, so change them here first.

**Label and paths** (`$HOME` is expanded when written; every path is absolute):
- `LABEL=com.killallgit.soze`
- Plist: `$HOME/Library/LaunchAgents/com.killallgit.soze.plist`
- Data: `$HOME/.local/share/soze/` with `bin/soze`, `bin/install.json`, `logs/soze.log`, `logs/soze.log.1`, `tap.jsonl`, `shapes.jsonl`, `debug.jsonl`
- Config: `$HOME/.config/soze/config.toml`
- launchd domain: `gui/$(id -u)`

**CLI:** `scripts/service.sh install <binary> | restart | reload | status | logs | uninstall [--purge]`. Anything else prints usage to stderr and exits 2, with no launchctl call and nothing written. The script calls `launchctl`, `git` and `tail` through `PATH`. It runs git from the repo root (`$(dirname "$0")/..`).

**install sequence:**
1. `<binary>` must be an executable regular file, or exit 2.
2. `mkdir -p` the data `bin`, data `logs`, `~/Library/LaunchAgents` and `~/.config/soze`.
3. `cp <binary> bin/soze.new`, `chmod 755`, `mv -f bin/soze.new bin/soze`. The rename gives the file a new inode; overwriting a running signed binary in place can get it killed on Apple Silicon.
4. Write `bin/install.json`.
5. Write `config.toml` only if it is absent.
6. If `logs/soze.log` exists, `mv -f` it to `logs/soze.log.1`.
7. Write the plist to `<plist>.new`, note whether it differs from the current plist, then `mv -f` it into place.
8. If `launchctl print gui/$UID/com.killallgit.soze` exits 0: run `launchctl kickstart -k gui/$UID/com.killallgit.soze` when the plist is unchanged, or `launchctl bootout gui/$UID/com.killallgit.soze` then `launchctl bootstrap gui/$UID <plist>` when it changed. Otherwise run `launchctl bootstrap gui/$UID <plist>`. A launchctl failure is the exit status.

**install.json** (exact text, one key per line):
```
{
  "source": "local",
  "sha": "<git rev-parse HEAD>",
  "branch": "<git rev-parse --abbrev-ref HEAD, with " escaped as \">",
  "dirty": <true if git status --porcelain prints anything, else false>,
  "installed_at": "<date -u +%Y-%m-%dT%H:%M:%SZ>"
}
```
If git fails, `sha` and `branch` are `unknown` and `dirty` is `false`.

**config.toml** (created only if absent, never overwritten):
```
log_format = "human"
live = true
```

**Plist** (exact text, two-space indent, `H` = `$HOME/.local/share/soze`):
```
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.killallgit.soze</string>
  <key>ProgramArguments</key>
  <array>
    <string>H/bin/soze</string>
    <string>--port</string>
    <string>48731</string>
    <string>--tap-file</string>
    <string>H/tap.jsonl</string>
    <string>--shape-file</string>
    <string>H/shapes.jsonl</string>
    <string>--log-file</string>
    <string>H/debug.jsonl</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>H/logs/soze.log</string>
  <key>StandardErrorPath</key>
  <string>H/logs/soze.log</string>
</dict>
</plist>
```

**Other commands:**
- `restart`: `launchctl kickstart -k gui/$UID/com.killallgit.soze`, and its status is the exit status.
- `reload`: `launchctl kill HUP gui/$UID/com.killallgit.soze`, and its status is the exit status.
- `status`: prints exactly these lines, then exits 0:
  ```
  state=<value from launchctl print, or "not loaded">
  pid=<value or empty>
  source=…
  sha=…
  branch=…
  dirty=…
  installed_at=…
  ```
  The last five lines appear only if `install.json` exists.
- `logs`: `exec tail -F "$HOME/.local/share/soze/logs/soze.log"`. It never names a tap, shape or debug path.
- `uninstall`:
  1. Run `launchctl bootout gui/$UID/com.killallgit.soze` and ignore its status.
  2. `rm -f` the plist, `bin/soze` and `bin/install.json`; `rmdir bin`.
  3. Keep the captures, `logs/` and `~/.config/soze`.
- `uninstall --purge`: after the steps above, refuse with exit 2 if `$HOME` is empty. Otherwise print every path under the data dir, one per line on stdout (paths only, never contents), then `rm -rf "$HOME/.local/share/soze"`. No confirmation prompt (design: lists, then deletes).

**Fake launchctl** (`$HOME/bin/launchctl`): appends `"$*"` (arguments joined by single spaces) as one line to `$HOME/launchctl.calls`.
- If `$HOME/launchctl.fail` holds this call's subcommand name, it exits 1.
- `print`: if `$HOME/launchctl.loaded` exists, prints `com.killallgit.soze = {`, a tab then `state = running`, a tab then `pid = 4242`, and `}`, then exits 0. Otherwise it exits 113.
- `bootstrap`: touches `launchctl.loaded`, exits 0.
- `bootout`: exits 0 and removes `launchctl.loaded` if it existed; otherwise exits 3.
- `kickstart` and `kill`: exit 0 if loaded, else 113.

**Fake git** (`$HOME/bin/git`), dispatching on `"$*"`:
- `rev-parse HEAD`: prints `0123456789abcdef0123456789abcdef01234567`
- `rev-parse --abbrev-ref HEAD`: prints `main`
- `status --porcelain`: prints ` M README.md` only if `$HOME/git.dirty` exists
- anything else: exits 1

**Fake tail** (kaiser-soze-4dn.4, service commands): writes its arguments one per line to `$HOME/tail.args` and exits 0.

**tests/service/lib.sh:**
- First line: `. "$(dirname "$0")/../wrappers/lib.sh"`. This reuses `ROOT`, `ORIG_PATH`, `RM`, `eq`, `case_run` and `finish`.
- It then redefines `new_home`: a fresh `mktemp -d` HOME, `$HOME/bin` first on PATH, `SOZE_PORT` unset, the fake launchctl and fake git written, and `touch "$HOME/.marker"`.
- Variables: `LABEL=com.killallgit.soze`, `DOMAIN="gui/$(id -u)"`, `DATA="$HOME/.local/share/soze"` and `PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"`, all set inside `new_home`.
- Helpers:
  - `service`: `sh "$ROOT/scripts/service.sh" "$@"`
  - `fake_soze <file> <tag>`: an executable `#!/bin/sh` file containing the line `# <tag>`
  - `loaded`: `touch "$HOME/launchctl.loaded"`
  - `calls`: prints `$HOME/launchctl.calls`
  - `expected_plist`: prints the plist above for the current HOME
  - `fake_tail`: added with kaiser-soze-4dn.4 (service commands)
- `tests/service/run.sh` is a copy of `tests/wrappers/run.sh`.

**Taskfile (kaiser-soze-4dn.5, Taskfile wiring):**
- `service:install`: `cargo build --quiet`, then `scripts/service.sh install "<binary path>"`. The binary is `<target_directory>/debug/soze`, with `target_directory` read from `cargo metadata --format-version 1 --no-deps`.
- `service:uninstall`: passes `{{.CLI_ARGS}}` (for `--purge`).
- The other four tasks each run `scripts/service.sh <command>`.

## Non-goals

- A Darwin check in the script. The tests also run on Linux, so the script does not refuse other systems.
- A CI job. No macOS job is added and `.github/workflows/ci.yml` is unchanged.
- A `service:update` task. `service:install` is the update path.
- A Rust `soze install` subcommand.
- Socket activation.
- Reading or tailing the tap, shape or debug file from the service commands.
- Deferred, each with its own bead:
  - `kaiser-soze-4dn.6` (tag-driven release workflow publishing checksummed binaries).
  - `kaiser-soze-4dn.7` (`soze update`: download, verify and install the latest release).
  - `kaiser-soze-4dn.8` (graceful drain of in-flight streams on SIGTERM for service restarts).
  - `kaiser-soze-6qt` (Linux systemd user unit).
  - A menu bar app.

## Requirements

- **R1**: The service never changes what the agent receives. Restart is the only interruption, and it is explicit.
- **R2**: Message content never reaches a terminal through a service command. `logs` reads `soze.log` only, and `uninstall --purge` prints paths, never contents.
- **R3**: Credential headers stay redacted before anything is written, as in `soze` itself. The script writes no credentials.
- **R4**: Every behavior above has a test first, with a fake `launchctl`, a fake `git` and a throwaway `HOME`. Fixtures are synthetic; no real traffic.
- **R5**: The script writes only under its `HOME`, and never under `~/.claude`, `~/.codex` or `~/.cursor`. It never installs the CA into a trust store.

## Decisions taken

1. **`--purge` has no confirmation prompt.** It lists every path under the data directory, then deletes it. The person typed `--purge`; a prompt would also make the script hard to test and to call from a task.
2. **A changed plist while loaded uses `bootout` then `bootstrap`.** `launchctl kickstart -k` restarts the process but does not re-read the plist, so a new flag or path would be ignored. An unchanged plist keeps the cheaper `kickstart -k`.
3. **Service tests are local only.** They run from `scripts/check.sh`, not from `ci.yml`. `ci.yml` runs on Linux only and has no macOS job; the fakes cover the script.
4. **Restart is abrupt.** In-flight streams are cut and a new `run_id` marks the restart. A graceful drain is deferred to `kaiser-soze-4dn.8` (graceful drain on SIGTERM).
5. **The binary is renamed into place.** Overwriting a running signed binary in place can get it killed on Apple Silicon, so the install copies to `bin/soze.new` and renames.
6. **Live settings live in `config.toml`, not in the plist.** The plist carries only the port and the output paths, so a `SIGHUP` reload can change the log format, level and `live` without reinstalling. `log_format` is restart-only in 008, so changing it needs `restart`.
7. **The binary path comes from `cargo metadata`.** `service:install` reads `target_directory` from `cargo metadata --format-version 1 --no-deps` and uses `<target_directory>/debug/soze`, so a shared Cargo target directory works where the literal `target/debug/soze` would not.
8. **The dev profile already has no debug symbols.** `Cargo.toml` sets `[profile.dev] debug = 0` and `incremental = false` (from #67, configuration versioning and reload), which keeps the target directory and the installed binary small. No `Cargo.toml` change is needed.

## Open

These are recorded, not decided.

1. **A `bootout` followed straight by `bootstrap` can race on real launchd.** `bootout` can return before the job is gone, so the `bootstrap` may fail. The fakes cannot show it. No fix is chosen; check it on a real install.
