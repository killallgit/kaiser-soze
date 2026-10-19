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
