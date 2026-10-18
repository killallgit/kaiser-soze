# kaiser-soze

Scope: [docs/objective.md](docs/objective.md).

A local explicit HTTP proxy that sits between a coding agent and its model API and taps the streamed results into a JSONL file. Responses are forwarded as they arrive; the tap reads a copy through a bounded queue and drops (and counts) events when it falls behind.

Hosts an adapter claims (Codex: `chatgpt.com`, `api.openai.com`; Claude Code: `api.anthropic.com`) are decrypted with a local CA. Every other host is tunnelled as raw TCP.

```sh
cargo build
./target/debug/soze --tap-file out.jsonl [--port 0] [--ca-dir ~/.config/soze]
```

The CA is created on first run in the CA dir (`ca.pem`, `ca-key.pem` mode 0600) and is name-constrained to the adapter hosts. It is never installed into a trust store. Point the agent at it:

```sh
HTTPS_PROXY=http://127.0.0.1:<port> CODEX_CA_CERTIFICATE=<ca-dir>/ca.pem codex exec ...
```

Claude Code (a native binary that honours `HTTPS_PROXY`):

```sh
HTTPS_PROXY=http://127.0.0.1:<port> NODE_EXTRA_CA_CERTS=<ca-dir>/ca.pem claude -p "..." </dev/null
```

On claimed exchanges the proxy removes the request's `accept-encoding` so streams arrive uncompressed and parseable. WebSocket `permessage-deflate` is left alone and inflated on the tap side. Responses that are still encoded, and request bodies over 32 MiB, are recorded as unparsed.

Each JSONL line holds `exchange`, `adapter`, `event`, `mono_us`, `wall_ms`, `start`, `end` and `payload`. Credential headers are redacted before anything is written.

A new tool is an `Adapter` (see `src/adapter.rs`): it names its hosts and, per request, returns an `ExchangeParser` or declines.
