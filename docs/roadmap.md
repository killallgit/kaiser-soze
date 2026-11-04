# Roadmap

Planned work is tracked in beads (`bd`). This file keeps shared terms and open questions.

## Terms

- **Harness**: the coding agent program that builds and sends model requests (`claude`, `codex`, `opencode`, `pi`). Named by its binary.
- **Provider**: the company or server that runs the model, and its hosts.
- **Provider API**: the contract a provider exposes (Anthropic Messages, OpenAI Responses, OpenAI Chat Completions). One API is spoken by many providers; one provider can offer several. The mapping is keyed by provider API and transport, never by harness.
- **Common scheme**: our stable, stripped-down schema ([common-format.md](common-format.md)). It keeps only what the model sees and says.
- **Wire**: what a harness actually sends and receives. A **frame** is one SSE event or WebSocket message.
- **Transport**: a built-in codec that cuts bytes into frames (`sse`, `websocket`, `json`).
- **Mapping**: a config file that turns wire into common scheme. A **rule** is one line of it. A **profile** is the config for one provider API at one version.
- **Engine**: compiled code that never takes behavior from config: forwarding, TLS, redaction, framing, assembly, the bounded queue.
- **Emit**: what leaves soze (a JSONL file today; a socket or other target later, undecided).

## Open questions

- Whether `CODEX_CA_CERTIFICATE` adds to or replaces the system roots.
- Which request header carries a usable version per provider API (Anthropic has an explicit one).
- Which harnesses allow a base URL by environment variable.
