# kaiser-soze

Scope: [docs/objective.md](docs/objective.md).

`soze` is a local HTTPS proxy between a coding agent and its model API. It forwards every byte as it arrives and taps a copy into a JSONL file in one common format ([docs/common-format.md](docs/common-format.md)). The tap never delays the agent: when it falls behind it drops events and counts them.

Hosts an adapter claims (Codex: `chatgpt.com`, `api.openai.com`; Claude Code: `api.anthropic.com`) are decrypted with a local CA. Every other host is tunnelled untouched.

## Use it

1. Start the proxy. It prints its port and the CA path.

   ```sh
   task dev            # dev proxy on port 48732; tap files under target/dev/, runs until Ctrl-C
   ```

   The dev proxy stays up until explicitly stopped; agents and consumer apps can use it concurrently. The shared proxy is the always-on service on 48731. There is no automatic timeout. Or run the binary: `soze --tap-file out.jsonl [--shape-file shapes.jsonl] [--log-file debug.jsonl] [--port 0]`. `--shape-file` writes the keys and types of each distinct request and reply event, never values. The CA is always `~/.config/soze/ca.pem`, made on first run.

2. Run an agent through it. The CA is created on first run, is name-constrained to the adapter hosts, and is never installed into a trust store. The agent trusts it through its own variable:

   ```sh
   HTTPS_PROXY=http://127.0.0.1:<port> CODEX_CA_CERTIFICATE=~/.config/soze/ca.pem codex exec ...
   HTTPS_PROXY=http://127.0.0.1:<port> NODE_EXTRA_CA_CERTS=~/.config/soze/ca.pem claude -p "..." </dev/null
   ```

3. Read the tap file. Message content is stored there and is never printed in a terminal, chat log or commit, so look at shapes and counts:

   ```sh
   jq -c '{exchange, event}' target/dev/tap.jsonl
   jq -c 'select(.event=="record") | {exchange, incomplete: .payload.incomplete, dropped: .payload.dropped}' target/dev/tap.jsonl
   ```

## Plain `claude`, `codex` and `agent`

Shell functions send each command through the proxy when it is running, with no arguments to remember. They work in zsh and bash.

```sh
task wrappers:install              # adds ~/.config/soze/wrappers.sh, then asks `[Y/n/e(dit)]` before adding one marked block to ~/.zshrc or ~/.bashrc (Enter or `y` adds it, `n` leaves it, `e` names another file)
task wrappers:install -- --dry-run # shows what would change, writes nothing
task wrappers:uninstall            # removes both; a backup of the start-up file is kept as <file>.soze-backup
task wrappers:install -- --yes     # skip the question
task wrappers:install -- --rc ~/.zshrc.local   # use another start-up file (or set SOZE_RC)
```

If the start-up file cannot be written (a managed or read-only `.zshrc`), or you answer no, the installer leaves it alone, installs only `~/.config/soze/wrappers.sh`, and prints the block to add by hand, in that file or any file your shell reads. The block does nothing if the wrappers file is missing.

Open a new shell, start the proxy on the wrappers' port (`soze --port 48731 ...`, or the always-on service), then run `claude`, `codex` or `agent` as usual.

- **They fail open.** The proxy is used only when something is listening on the port, the CA file is readable, and `HTTPS_PROXY` is not already set. In every other case, including soze not running or broken, the real command runs exactly as without the wrappers.
- **Bypass one command** with `SOZE_OFF=1 claude` or `command claude`.
- **Without shell functions** (a start-up file you cannot edit): `task wrappers:install` also copies `soze-wrap` to `~/.local/bin`, so `soze-wrap claude -p "..."` runs one command through the same wrappers. Put `~/.local/bin` on `PATH`.
- **Settings:** `SOZE_PORT` (default 48731, the always-on service port; `task dev` listens on 48732, so `SOZE_PORT=48732 claude` reaches the dev build). The CA is always `~/.config/soze/ca.pem`, made on first run, so `task dev` and the wrappers need no setting.
- **Known limit:** the proxy address is fixed when the agent starts. If soze stops mid-session, that session loses its connection; start a new one.
- `agent` (Cursor) gets the proxy only: its host is tunnelled, so nothing is parsed from it.

## The always-on service (macOS)

A launchd user agent keeps one `soze` running on port 48731, the port the wrappers use. `task dev` listens on 48732, so both can run at once.

```sh
task service:install     # build the debug binary, copy it into place, rotate the log, (re)start
task service:status      # state, pid, and the sha, branch, dirty flag and time of the install
task service:logs        # tail -F soze.log
task service:reload      # SIGHUP: re-read the live logging settings
task service:restart     # kickstart the service
task service:uninstall   # stop and remove the service and binary
```

Layout, all under your home directory:

| Path | Holds |
| --- | --- |
| `~/Library/LaunchAgents/com.killallgit.soze.plist` | the launchd job, label `com.killallgit.soze` |
| `~/.local/share/soze/bin/soze`, `bin/install.json` | the installed binary and what it was built from |
| `~/.local/share/soze/logs/soze.log`, `soze.log.1` | the service's stdout and stderr, and the previous generation |
| `~/.local/share/soze/tap.jsonl`, `shapes.jsonl`, `debug.jsonl` | the tap, shape and debug files |
| `~/.config/soze/config.toml` | settings; written on first install with `log_format = "human"` and `live = true`, never overwritten |

- **Update the service** by re-running `task service:install`. It rebuilds the debug binary, copies it over the installed one, moves `soze.log` to `soze.log.1`, and restarts the service.
- **Restart is abrupt.** In-flight streams are cut and agents must reconnect. The new process has a new `run_id`, which marks the restart in the tap.
- **Logs.** `task service:logs` tails `soze.log` only. It never reads the tap, because message content must not reach a terminal.
- **Uninstall** removes the plist, binary and `install.json`, and keeps captures, logs and config. `task service:uninstall -- --purge` also lists every path under `~/.local/share/soze/`, then deletes the directory; `~/.config/soze` stays.
- **`live = true` in `config.toml`** applies to `task dev` too, so `LIVE=0 task dev` no longer turns live lines off. Pass `-- --live=false`, or edit the file.
- **Smoke test.** `task test:smoke` checks that the installed service is running, accepts connections, proxies one `claude -p` call and captures it in the tap without new `connect_failed` lines; it needs the installed service and a logged-in `claude`, and prints counts and ids only.
- **Tests.** The service tests in `tests/service/` are local shell tests with a fake `launchctl`. `task check` runs them; CI does not.

## Configuration

`soze --help` lists the options, and `soze --version` prints the version. Arguments are parsed with clap; config-rs loads and merges TOML settings at startup.

By default, soze reads `/etc/soze/config.toml`, then `~/.config/soze/config.toml`. Missing files are skipped; existing unreadable files or malformed TOML fail startup. `--config <FILE>` reads that file instead of the default locations and requires it to exist.

Values resolve in this order, highest priority first: explicit flags, declared environment variables, user file, system file, built-in defaults. Only `SOZE_LOG` (for `log_level`) and `SOZE_LOG_FORMAT` (for `log_format`) are binary environment settings. `SOZE_PORT` remains a wrapper/dev-script setting. When using `--config`, its file occupies the file position in this order.

```toml
# ~/.config/soze/config.toml, or a file selected with --config
port = 48731
tap_file = "out.jsonl"
# shape_file = "shapes.jsonl"
# log_file = "debug.jsonl"
log_level = "info"
log_format = "human"
live = true
stream = false
```

`tap_file` must be supplied by a file or flag. Unknown keys and invalid winning values are rejected; a valid higher-priority flag may override an invalid lower-priority environment value. Relative paths use the process working directory. Capture and debug outputs cannot alias the configuration inputs; the existing debug-versus-capture alias protection also applies after merging.

Bare `--live` and `--stream` enable their setting; `--live=false` and `--stream=false` disable a configured value. Repeating a valued option uses its last value, so flags forwarded by `task dev -- ...` still override the script's defaults. An explicit log level wins over the stream-dependent default (`debug` with streaming, `info` otherwise); the log-format default is human on a terminal and JSON otherwise. The dev script's explicit capture paths, port and live flag (unless `LIVE=0`) take precedence over config files.

Configuration is loaded at startup. SIGHUP reloads the live logging settings; every other key, including `[retention]`, takes effect on restart.

## The live view

`task dev` prints compact request/reply metadata. Add `--stream` to see events as the stream arrives, including block indexes, delta field names and byte counts. This observes capture before disk writes; a terminal line is not proof that its tap record was persisted.

```sh
task dev:stream                              # compact real-time metadata, debug level by default
task dev:stream:json                         # structured stderr for collection
task dev -- --log-level warn                 # warnings and errors only
task dev -- --live=false                      # operational logs without request/reply summaries
```

Human output uses UTC time, a short run id, exchange id, adapter and a few metadata keys, capped at 160 characters per line. JSON keeps the full run id and numeric metadata in valid records of at most 1 KiB, excluding the newline. Message, system and reasoning text, tool names/inputs/outputs, headers and credentials are never included. For example, a delta is identified by its block index, field name and byte count; its text is not previewed.

`messages` counts conversation entries in the model request, including any system/context entries and history; it does not count prompts you typed. `tools` counts available tool definitions, not executions. Structured records also give `user_messages`, `assistant_messages`, `system_messages`, `other_messages`, and `tool_calls` (calls present in request history). `unsupported` means a claimed host used an endpoint the adapter does not parse; `run=…` is a separate capture id. `adapter=none` on a `tunnel` means no adapter claims that host, so traffic passes through without decryption or parsing. These decisions do not prevent forwarding.

Levels are `off`, `error`, `warn`, `info` and `debug`. `--log-level` overrides `SOZE_LOG`; otherwise the default is `info`, or `debug` with `--stream`. An explicitly selected level is respected even with `--stream`. `--log-format human|json` overrides `SOZE_LOG_FORMAT`; the default is human for a terminal and JSON otherwise. The binary accepts these same flags; `--live` enables request/reply summaries independently of `--stream`.

Terminal stream detail has a budget of 20 records per one-second window; summaries and non-error operational events have a separate budget of 20. Error diagnostics bypass the rate filter but still use the bounded 256-record queue. A subsequent accepted record reports `suppressed` counts; queue/oversize losses are reported as a `log_dropped` event. A blocked or broken log destination never delays forwarding or the tap. Small startup-failure logs get a bounded drain before exit.

Every dev task also writes `target/dev/debug.jsonl` through a separate logger transport. It resets when the service starts, always selects debug metadata (even with terminal logging off), and does not apply the terminal rate filter. It includes full run/exchange ids, role counts, status/timing/loss counters, and adapter `decision` reasons. `endpoint` is a known API route or `other`; arbitrary paths and query values are excluded. New files are created with mode `0600`. The binary enables this transport with `--log-file <path>`; its parent directory must already exist. Starting a second service on an occupied port leaves the active debug file intact. A debug file cannot alias the tap or shape file, including through dangling symbolic links.

The debug file stops accepting records at 16 MiB per run and reports `debug_file_limit` on stderr; it resumes after the next start resets the file. Open/write/flush failures also report safe diagnostics on stderr while the proxy keeps running. Its independent 256-record queue can drop under load, and abrupt shutdown may lose queued records. This is a disposable debug log, not a durable audit trail. The tap and shape files are bounded separately (see [The tap file](#the-tap-file)); the debug file and `soze.log` (launchd stderr, rotated one generation by the service install) are excluded from that retention.

```sh
tail -f target/dev/debug.jsonl               # metadata only; safe to inspect while developing
```

Operational diagnostics go to stderr independently of the tap file. Disk open/write/flush failures, TLS/upstream failures and connection failures use fixed event names and safe error categories, never raw error text. Process exit after startup failure stays bounded even if stderr is blocked. A service supervisor can collect stderr; log retention is separate from capture retention. The shared dev service continues running until explicitly stopped.

## The tap file

Each line has `run_id`, `schema_version`, `sequence`, `exchange`, `adapter`, `event`, `mono_us`, `wall_ms`, `start`, `end` and `payload`. Credential headers are redacted before anything is written.

The tap file and the shape file are bounded on disk. A closed segment is named by inserting a six-digit index before the extension: `tap.jsonl` becomes `tap.000042.jsonl`. The first index is 1, the next is the highest existing index plus 1, and an index is never reused. The active file keeps its plain name and is created with mode `0600`.

```toml
[retention]
segment_mib = 64
capacity_mib = 1024
max_age_days = 14
```

These keys apply to the tap, are read at startup only (not reloaded on SIGHUP), must be positive, and `capacity_mib` must be at least `segment_mib`. The values above are the defaults. The shape file uses fixed limits of 4 MiB per segment and 32 MiB in total, with the same 14 days, and has no keys.

- **Rotation.** Only a regular file rotates. Before a line is written, if the active file is non-empty and the line would push it past the segment limit, the active file is closed as the next numbered segment and a new one is opened. Rotation also happens at every start when the active file is a non-empty regular file, so each closed tap segment holds one `run_id`, and `sequence` restarts at 1 in the new active file. If a rotation fails, soze logs `sink_rotate_failed` and keeps appending to the current file.
- **Cleanup.** After each successful rotation, the oldest closed segments are deleted while the segments plus the active file exceed the capacity, or while the oldest segment is older than the maximum age. The active file is never deleted, and a pass stops at the first failed delete. Age is checked only at a rotation, not by a periodic sweep, so an idle proxy keeps its files. `retention_deleted` (Info, with `segments` deleted and the highest `segment` index) and `retention_delete_failed` (Warn, with `segment`) report each pass; they carry an error kind, never message content.
- **Oversized history at start.** A previous run's file larger than the capacity is closed and then deleted in the same pass, so a first start with a smaller capacity discards it.
- **Expired history.** Nothing in the file marks it. A reader lists the segments to find the oldest remaining one and sees a gap in `run_id` or `sequence`. The explicit resync marker belongs to the planned local stream.
- **Guard.** Startup refuses a shape, debug or config path named like a closed segment of the tap or shape file in the same directory (`output_overlaps_segment`).

- **Live events** are written as each chunk arrives: `conversation`, `block_start`, `block_delta`, `block_stop`, `usage`, `reply_end`, `error`.
- **`record`** is one line per reply, written when it ends, with the whole conversation and reply. `incomplete` and `dropped` are set when events were lost or the stream was cut off.
- **`edge`** is one relation between nodes (`project`, `session`, `thread`, `exchange`, `step`, `turn`, `call`, `result`) taken from request headers and ids, never message text: `in_project`, `in_session`, `in_thread`, `spawned`, `turn`, `continues`, `calls`. See [the graph](docs/common-format.md#the-graph).
- **Images** are a reference, content type and size, never the bytes.
- **`unsupported`** marks a request on a claimed host that the adapter does not read (host and path only). **`unparsed`** marks a body that could not be read: still encoded, or over 32 MiB.

Line shapes: [specs/002-agent-traffic-capture/contracts/records.md](specs/002-agent-traffic-capture/contracts/records.md). To support a new tool, implement the `Adapter` trait: name its hosts and return a parser per request.

## Tasks

Run `task --list`. Tasks run from any directory in the repo.

- `task check`: formatting, clippy with warnings as errors, and all tests. CI runs the same checks.
- `task test:unit -- <module>::`: unit tests; the module filter is optional.
- `task test:integration`: integration tests only.
- `task dev`: see above.
- `task dev:stream`: real-time metadata in compact terminal format.
- `task dev:stream:json`: real-time metadata as JSON logs.
- `task test:smoke`: one real `claude` call through the installed always-on service; local only, not part of `task check`.
- `task test:wrappers`: shell tests for the wrappers and their installer, run under every shell installed. `task check` runs them too.
- `task service:install`, `service:status`, `service:logs`, `service:reload`, `service:restart`, `service:uninstall`: the always-on macOS service, see above.

## Build disk use

The dev profile disables debug information and incremental compilation; tests inherit those settings. Builds keep debug assertions and overflow checks. Rebuilding changed Rust code does more work without the incremental cache, while Cargo still reuses compiled dependencies.

Cargo respects your configured target directory, including a shared one. Use `cargo clean -p kaiser-soze` for package-specific cleanup. Cargo's automatic global-cache cleanup covers unused downloaded dependencies and extracted sources, not build artifacts. `target/dev/` holds capture data and must be preserved when cleaning build output.
