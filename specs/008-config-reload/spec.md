# Feature Specification: Configuration versioning and safe live reload

**Status**: Implemented. The open decisions below were settled as drafted.
**Input**: Bead `kaiser-soze-96d` (specify configuration versioning and safe live reload), a follow-up to `kaiser-soze-bqo.15` (clap and layered startup configuration). The roadmap item is [Configuration library](../../docs/roadmap.md): "A bad file keeps the old config and reports; it never stops capture." This spec is the policy. Adding `config-rs` did not implement any of it.

## Problem

Configuration is read once, at startup, by `config::load` (defaults, then the system and user files, then environment, then flags). Nothing says what a file edit means to a running `soze`, and the file has no version, so a future key cannot be introduced or retired safely.

What the code does today, which the policy has to fit:

- `Settings` has eight keys: `port`, `tap_file`, `shape_file`, `log_file`, `log_level`, `log_format`, `live`, `stream`. Unknown keys are an error (`deny_unknown_fields`).
- `port` is bound once; `tap_file`, `shape_file` and `log_file` are opened once (the debug file is truncated on open).
- `Logger` reads `level`, `live` and `stream` from its settings for every event, but `format` is copied into the writer task when the logger starts.
- `Sources` captures files, environment and the format default once, so tests and startup resolve the same inputs.
- Validation (`paths::validate`) does filesystem calls: it rejects a debug log that is the same file as the tap or shape file, and any output that is a loaded config file.
- `tokio` is pinned without the `signal` feature; `sync` is enabled.

## User Story 1 (P1): A bad reload changes nothing

A person edits the config file while `soze` is capturing, then asks for a reload. If the file is wrong in any way, `soze` keeps running with the configuration it had, forwards the same bytes, and reports why.

**Independent test**: start with a valid configuration, send a reload with a file that is unreadable, one with invalid TOML, one with an unknown key, one with a newer `version`, and one whose output path overlaps another output. After each, the running configuration is the one before, and a request forwarded across the reload arrives byte for byte.

**Acceptance**
1. A candidate is built and validated off to the side. Nothing is published unless every check passes.
2. A failed reload writes one `config_reload_failed` operational log event carrying only an error kind from the existing vocabulary (`invalid_configuration`, `missing_configuration`, `debug_file_overlaps_capture`, `output_overlaps_configuration`) or `unsupported_version`. No file contents, no values.
3. A failed reload never stops, restarts or delays the listener, a connection in flight or the tap.
4. A file that disappears after startup is a failed reload (the last known good stays), not an empty configuration.
5. At startup the same errors still stop `soze` with `service_start_failed`, as today. Only a running process has a last known good to keep.

## User Story 2 (P1): Reload what is safe, say what needs a restart

**Which settings change live**

| Key | On reload | Why |
|---|---|---|
| `log_level` | live | read per event |
| `live` | live | read per event |
| `stream` | live | read per event |
| `log_format` | restart | copied into the writer task at start; changing it needs the writer to take it over a channel. Not worth it in the first version |
| `port` | restart | the listener is bound once; harnesses and the shell wrappers hold the port |
| `tap_file` | restart | opened once; swapping it is rotation, which belongs to disk retention (`kaiser-soze-x07`, bound disk retention for the shared tap output) |
| `shape_file` | restart | same as `tap_file` |
| `log_file` | restart | opened and truncated once; a reload must never erase a live run's log |

**Independent test**: reload a file that changes `log_level` and `port`. The new level is in effect for the next event. The listener keeps its port. One `config_restart_required` event names `port`.

**Acceptance**
1. A reload that changes a live key publishes it atomically with the rest of the candidate. An exchange reads one snapshot when it starts and never sees a mix of old and new values.
2. A reload that changes a restart-only key still publishes the live keys; the running value of the restart-only key stays in force. The operational log gets `config_restart_required` with the key names (names only, never values).
3. A candidate that changes nothing effective publishes nothing and logs nothing.
4. Environment and flags are captured at startup and apply to every reload, so they keep their precedence: a file edit to a key a flag sets has no effect, and is not reported as a change.
5. `--config <path>` is re-read from that path. Without it the same system and user files are re-read; a file that did not exist at startup is picked up if it exists later.
6. `Settings::reload_into` names every restart-only key in one list and the live keys in one assignment, so a new key is not reloaded or reported until it is added there.

## User Story 3 (P1): Versioned files

**Independent test**: a file with no `version`, with `version = 1`, with `version = 2` (unknown to this build) and with `version = "one"`. The first two load identically; the others are rejected as `unsupported_version` or `invalid_configuration`, and a running process keeps its last known good.

**Acceptance**
1. A file may set a top-level integer `version`. Absent means `1`. The version belongs to files only; environment and flags are not versioned.
2. A version higher than this build supports is rejected, never read as the nearest known version. A build never guesses at a newer schema.
3. Unknown keys stay an error. A newer file on an older `soze` fails loudly instead of silently dropping settings.
4. Each schema change that renames or removes a key adds one step that rewrites the parsed table from version *n* to *n + 1* before deserializing. An older file therefore keeps working on a newer build. `soze` never rewrites a user's file.
5. Layers merge after each file is migrated, so a version 1 user file and a version 2 system file combine correctly.
6. A key is retired by one build that warns (`config_key_deprecated`, key name only) and a later build that migrates it away. Removing a key and its migration in one build is not allowed.

## User Story 4 (P2): Reload is asked for, never polled

**Independent test**: send `SIGHUP` three times in quick succession during a reload. One further reload runs afterwards; none runs concurrently; forwarding is unaffected throughout.

**Acceptance**
1. The trigger is `SIGHUP`, handled by one task. It is installed only when stderr is not a terminal: closing the terminal a foreground `soze` runs in must still stop it. `systemctl reload` and `launchctl kill HUP` (the service work in roadmap item 6) reach it with no extra code.
2. There is no file watcher in the first version. Editors save by rename and in several steps, a watcher needs a new dependency, and an explicit trigger makes "I changed the file" and "apply it" the same moment.
3. Signals that arrive during a reload coalesce into one more reload afterwards.
4. The reload task reads and validates in `spawn_blocking`; the validation does filesystem calls and must not run on a forwarding thread.
5. `soze` has no reload command-line subcommand in this version (the process is addressed by signal).

## Design

- **`Settings::reload_into(candidate)`** copies the live keys (`log_level`, `live`, `stream`) from the candidate and returns the names of the restart-only keys that differ, which keep their running value.
- **`cli::Reload`** keeps the parsed flags and the captured `Sources`, so `Reload::run` is `config::load` again under the same precedence. Startup and reload cannot disagree.
- **`Logger::reload`** replaces `level`, `live` and `stream` of the terminal output through a `tokio::sync::watch` sender; `policy.rs` reads the current value per event. The writer keeps `format`, and the debug file output is skipped.
- **`reload_on_hangup`** in `main.rs` waits for `SIGHUP`, runs `Reload::run` in `spawn_blocking`, then calls `reload_into` and `Logger::reload`. The signal handler is installed before `listening` is logged, so a reload sent at once is never the default `SIGHUP` action.
- **Dependency**: the existing `tokio` pin gains its `signal` feature. That adds one transitive crate, `signal-hook-registry` 1.4.8 (published 2025-12-25), and no new direct dependency.

## Non-goals

- A file watcher, a control socket or an HTTP reload endpoint.
- Rotating or swapping `tap_file`, `shape_file` or `log_file` live.
- Changing `port` live.
- Live `log_format`.
- Per-profile and mapping reload (roadmap item 5); it will reuse the same candidate, publish and last-known-good rules but has its own spec.
- Writing the effective configuration back to disk.

## Requirements

- **R1**: Forwarding never waits on a reload. A reload that is slow, failing or panicking drops itself, logs, and changes no forwarded byte.
- **R2**: Publication is atomic: one `send` of one immutable value.
- **R3**: Operational log events from a reload carry key names and error kinds, never values or file contents.
- **R4**: Every behavior above has a test first, and the fixtures are synthetic.

## Decisions taken

1. **Partial apply (Story 2, acceptance 2).** A reload publishes the live keys even when the same file changes a restart-only key, and reports the restart-only key. One wrong `port` should not block a `log_level` change, and capture is safe either way. The cost is that the file and the running process can disagree until a restart.
2. **`log_format` live.** Restart-only. Making it live means the writer task receives format changes over a channel.
3. **Retiring a key (Story 3, acceptance 6).** One build warns before a later build removes the key. Nothing in the code needs it yet; it is a rule for the first key that is retired.
