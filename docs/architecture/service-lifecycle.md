# Service lifecycle and storage

The installed Soze service is a local proxy owned by one user. It runs the foreground binary
under an OS supervisor, keeps agent content in that user's files and does not change system
proxy or certificate-trust settings. See the [system overview](system-overview.md) for the
architecture diagram and capture pipeline.

## Platform support

`task service:*` manages a macOS LaunchAgent or a Linux systemd user unit. Linux requires
systemd 240 or newer (for file-log append support), a reachable user manager/session bus
and `/bin/kill` for reload (`procps` on Debian).
The installer checks manager access and unit-directory visibility before changing installed
files. A custom `XDG_CONFIG_HOME` must agree with the running user manager's unit search path;
an override visible only to the invoking shell is rejected. No root access is needed.

| Concern | macOS | Linux |
| --- | --- | --- |
| Supervisor | `launchd` user LaunchAgent | `systemd --user` service |
| Definition | `~/Library/LaunchAgents/com.killallgit.soze.plist` | `${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/soze.service` |
| Startup | GUI user login; job uses `RunAtLoad` | User-manager startup, normally login |
| Automatic recovery | `KeepAlive` | `Restart=on-failure`, five-second delay |
| Run without a login session | Not supplied by the current GUI-domain installer | Explicitly enable user lingering |
| Logging reload | SIGHUP | SIGHUP through `ExecReload` |

The current macOS installer targets `gui/<uid>`, not a machine-level LaunchDaemon. It does not
provide unattended boot-time operation on a headless Mac. For Linux without systemd, no
service backend is implemented; the foreground binary can be managed by another supervisor.

One service per user avoids sharing CA private keys and captured conversations across users.
The default port is still shared by the machine: two users cannot both bind
`127.0.0.1:48731`. Multi-user deployment would need distinct ports and matching wrapper
settings; both installers currently fix the service port at 48731.

## Service operations

Run these commands from a checkout with Task installed:

```sh
task service:install     # build a debug binary, copy it into place and (re)start
task service:status      # supervisor state, PID and installed-build provenance
task service:logs        # follow the service log, never the capture files
task service:reload      # SIGHUP: reload live logging settings
task service:restart     # abruptly restart the process
task service:uninstall   # remove the job and binary; keep captures, logs and config
```

Linux installation writes `soze.service`, reloads the user manager, enables the unit for
`default.target` and restarts it. It runs the foreground binary continuously, not through
socket activation. stdout and stderr append to the same `soze.log` used on macOS; the logs
command does not read journald or capture output. New Linux directories and non-executable
files are private to the user, and the unit applies `UMask=0077` to runtime files.

To keep the Linux user manager running from boot and after logout, explicitly enable
lingering with `loginctl enable-linger "$USER"`; authorization depends on local policy.
Installation and removal never change lingering, which can also support other user services.
Without it, service lifetime follows the system's user-manager/session policy.

Installation copies the binary to `~/.local/share/soze/bin/soze`, so changing checkout branches
or rebuilding a development binary does not replace the running installation. Re-running
`task service:install` updates the installed binary and restarts the service. It records the
source SHA, branch, dirty flag and installation time in `bin/install.json`.

Restart and update cut in-flight streams. The supervisor can restore the service, but it does
not restore those connections or transparently move an existing agent onto a direct route.
A new process receives a fresh capture `run_id`.

SIGHUP reloads live logging settings only. Other configuration changes, including retention,
require a restart. The installer's explicit port and output-path flags take precedence over
configuration-file values. See [configuration](../../README.md#configuration) for precedence
and the complete option contract.

The shared installed service uses port 48731; `task dev` uses 48732, so both can run at once.
Use `SOZE_PORT=48732` when launching a wrapped agent against the development proxy.

## Installed file layout

| Path | Contents |
| --- | --- |
| `~/Library/LaunchAgents/com.killallgit.soze.plist` | macOS supervisor definition |
| `~/.config/systemd/user/soze.service` | Linux supervisor definition; follows `XDG_CONFIG_HOME` if set |
| `~/.local/share/soze/bin/soze` | Installed executable |
| `~/.local/share/soze/bin/install.json` | Installed-build provenance |
| `~/.local/share/soze/tap.jsonl` and numbered segments | Captured events, content, records and graph edges |
| `~/.local/share/soze/shapes.jsonl` and numbered segments | Request/reply keys and types without values |
| `~/.local/share/soze/debug.jsonl` | Disposable debug metadata |
| `~/.local/share/soze/logs/soze.log`, `soze.log.1` | Service stdout/stderr and the previous generation |
| `~/.config/soze/config.toml` | User configuration |
| `~/.config/soze/` | Local CA certificate and private key, alongside configuration |

The first install creates `config.toml` with `log_format = "human"` and `live = true` if the
file is absent. Later installs preserve it. The CA is made on the binary's first run, not
installed into a trust store.

Normal uninstall keeps captures, logs, configuration and CA material. On Linux it first
verifies that the caller's unit path matches the manager's loaded definition, then disables
and stops the unit before deleting it and reloading the manager. A mismatched path or stop
failure aborts removal, so files are not deleted from underneath a possibly running service.
`task service:uninstall -- --purge` additionally lists and deletes the entire
`~/.local/share/soze/` directory. It still keeps `~/.config/soze/`.

## Storage and retention

| Output | Current policy |
| --- | --- |
| Tap captures | Defaults: 64 MiB segments, 1024 MiB total capacity, 14 days; configurable under `[retention]` |
| Shape output | Fixed: 4 MiB segments, 32 MiB total capacity, 14 days |
| Debug metadata | Stops accepting records at 16 MiB per run; reset on the next start |
| Service log | One previous generation retained when the service installer rotates it; no continuous size-based rotation |

Tap and shape files rotate before a line would exceed the segment size, when the active file
is non-empty. A non-empty active file also rotates at startup. Cleanup runs after successful
rotation and removes the oldest closed segments by capacity or age; it never deletes the
active file. An idle process does not periodically expire files, so age and capacity settings
are not continuously enforced hard ceilings. See the [tap-file
contract](../../README.md#the-tap-file) for failure behavior and oversized-file handling.

Agent sessions are logical identities within captures, not directories or files. Several
sessions can share a segment, and one session can span several segments and capture runs.
Retention removes segments, not whole sessions, so the retained portion of a session can be
incomplete. Session counts and per-session sizes cannot be inferred from file counts.

## Management and visibility

The `soze` CLI starts the proxy; it has no `status`, storage-usage or session-listing
subcommands and exposes no management or health API.

| Information | Available interface |
| --- | --- |
| Supervisor state and PID | `task service:status`, macOS and Linux |
| Installed source SHA, branch, dirty flag and installation time | Same command, from `bin/install.json` |
| Content-free operational activity | `task service:logs` |
| Capture and log disk usage | Filesystem tools; no built-in aggregate report |
| Retained session count or per-session size | No built-in report or persistent session index |

Supervisor state and a listening TCP port do not prove successful upstream requests or
complete capture. The existing status command reports the supervisor and installation, not
an application-level health assessment.

Linux status maps systemd's service substate to `state` (for example, `running`, `dead` or
`failed`), reports an empty PID when no process is running, and uses `not loaded` for a missing
unit. An unreachable user manager fails the command rather than looking like an absent unit.

## Testing supervision

`task check` and Linux CI run isolated shell tests of both service backends using fake OS
commands. `task test:systemd` separately builds a debug Linux binary and boots a disposable
systemd Docker image, exercising the real installer and proxy under a non-root user manager.
It checks enablement, state/PID, proxy forwarding, reload, restart, automatic recovery,
update and retained data after uninstall, followed by purge.

Docker is only the integration-test environment; it does not replace native host supervision.
The test needs a trusted rootful Linux Docker daemon with cgroup v2 and uses a privileged
container with a private cgroup namespace. No host directories or sockets are mounted, no
ports are published and runtime networking is loopback-only. The container and its generated
capture/CA files are removed after the test. See the [Docker test
instructions](../../README.md#test-real-systemd-with-docker) before running against a local or
remote Docker context.

## Inspecting disk usage

These commands report disk usage without printing captured content:

```sh
du -sh "$HOME/.local/share/soze"
du -sh "$HOME/.local/share/soze/logs"
```

The first total includes the installed binary, captures, shape output, debug output and
service logs. The second isolates the service-log directory. Disk allocation reported by `du`
is not an exact serialized byte count for a session.

A future cross-platform status command could report service state, build, port, disk usage
and retention settings, even when the proxy is stopped. Session summaries would require
metadata scanning or an index. Neither interface is implemented; no new API, index or
transport is assumed here.
