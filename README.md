# kaiser-soze

Scope: [docs/objective.md](docs/objective.md).

`soze` is a local HTTPS proxy between a coding agent and its model API. It forwards every byte as it arrives and taps a copy into a JSONL file in one common format ([docs/common-format.md](docs/common-format.md)). The tap never delays the agent: when it falls behind it drops events and counts them.

Hosts an adapter claims (Codex: `chatgpt.com`, `api.openai.com`; Claude Code: `api.anthropic.com`) are decrypted with a local CA. Every other host is tunnelled untouched.

## Use it

1. Start the proxy. It prints its port and the CA path.

   ```sh
   task dev            # tap file and CA under target/dev/, stops after DEV_TIMEOUT seconds (default 300, max 600)
   ```

   Or run the binary: `soze --tap-file out.jsonl [--shape-file shapes.jsonl] [--port 0] [--ca-dir ~/.config/soze]`. `--shape-file` writes the keys and types of each distinct request and reply event, never values.

2. Run an agent through it. The CA is created on first run, is name-constrained to the adapter hosts, and is never installed into a trust store. The agent trusts it through its own variable:

   ```sh
   HTTPS_PROXY=http://127.0.0.1:<port> CODEX_CA_CERTIFICATE=<ca-dir>/ca.pem codex exec ...
   HTTPS_PROXY=http://127.0.0.1:<port> NODE_EXTRA_CA_CERTS=<ca-dir>/ca.pem claude -p "..." </dev/null
   ```

3. Read the tap file. Message content is stored there and is never printed in a terminal, chat log or commit, so look at shapes and counts:

   ```sh
   jq -c '{exchange, event}' target/dev/tap.jsonl
   jq -c 'select(.event=="record") | {exchange, incomplete: .payload.incomplete, dropped: .payload.dropped}' target/dev/tap.jsonl
   ```

## Plain `claude`, `codex` and `agent`

Shell functions send each command through the proxy when it is running, with no arguments to remember. They work in zsh and bash.

```sh
task wrappers:install              # adds ~/.config/soze/wrappers.sh and one marked block in ~/.zshrc or ~/.bashrc
task wrappers:install -- --dry-run # shows what would change, writes nothing
task wrappers:uninstall            # removes both; a backup of the start-up file is kept as <file>.soze-backup
```

Open a new shell, start the proxy on the wrappers' port (`task dev` uses it, or `soze --port 48731 ...`), then run `claude`, `codex` or `agent` as usual.

- **They fail open.** The proxy is used only when something is listening on the port, the CA file is readable, and `HTTPS_PROXY` is not already set. In every other case, including soze not running or broken, the real command runs exactly as without the wrappers.
- **Bypass one command** with `SOZE_OFF=1 claude` or `command claude`.
- **Settings:** `SOZE_PORT` (default 48731) and `SOZE_CA_DIR` (default `~/.config/soze`). `task dev` keeps its CA under `target/dev/ca`, so use `SOZE_CA_DIR=$PWD/target/dev/ca claude` while developing.
- **Known limit:** the proxy address is fixed when the agent starts. If soze stops mid-session, that session loses its connection; start a new one.
- `agent` (Cursor) gets the proxy only: its host is tunnelled, so nothing is parsed from it.

## The live view

`task dev` prints one short line per request and per reply, so you can see traffic arriving without reading the tap file. `LIVE=0 task dev` turns it off; `soze --live` turns it on for the binary. Lines go to standard error and never hold message text, system text or tool names and inputs: only counts, token numbers, the stop reason, block kinds, and the agent command and directory.

```
12:03:41  #3 codex  -> 7 msgs  4 tools  warmup  ~/Code/kaiser-soze
12:03:44  #3 codex  <- completed  2.1s  in 1203 out 88  text,tool_call
12:03:45  #5 none  tunnel api2.cursor.sh:443  agent
```

The directory comes from the wrappers: they set the proxy address to `http://<command>:<directory>@127.0.0.1:<port>` and `soze` reads it from the connection. A missing or malformed label is ignored and never affects the connection. Claude Code, Codex and Cursor's `agent` were each checked and all pass the label on. If a terminal stalls, lines are dropped and counted, and the tap file and forwarding are not delayed.

## The tap file

Each line has `exchange`, `adapter`, `event`, `mono_us`, `wall_ms`, `start`, `end` and `payload`. Credential headers are redacted before anything is written.

- **Live events** are written as each chunk arrives: `conversation`, `block_start`, `block_delta`, `block_stop`, `usage`, `reply_end`, `error`.
- **`record`** is one line per reply, written when it ends, with the whole conversation and reply. `incomplete` and `dropped` are set when events were lost or the stream was cut off.
- **Images** are a reference, content type and size, never the bytes.
- **`unsupported`** marks a request on a claimed host that the adapter does not read (host and path only). **`unparsed`** marks a body that could not be read: still encoded, or over 32 MiB.

Line shapes: [specs/002-agent-traffic-capture/contracts/records.md](specs/002-agent-traffic-capture/contracts/records.md). To support a new tool, implement the `Adapter` trait: name its hosts and return a parser per request.

## Tasks

Run `task --list`. Tasks run from any directory in the repo.

- `task check`: formatting, clippy with warnings as errors, and all tests. CI runs the same checks.
- `task test:unit -- <module>::`: unit tests; the module filter is optional.
- `task test:integration`: integration tests only.
- `task dev`: see above.
- `task test:wrappers`: shell tests for the wrappers and their installer, run under every shell installed. `task check` runs them too.
