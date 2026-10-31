# Roadmap

High level only. Each item becomes its own feature and spec when it is next. Order is a proposal.

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

## Items

1. **Traffic survey.** Run each harness and provider pair with a harmless prompt (first pass: `claude` on Anthropic and `codex` on OpenAI Responses; the rest later) and record only shapes: host, path, transport, event types, which common scheme fields are present. Decides which mappings exist. No content.
2. **Architecture and module conventions.** Precise description of the structure, boundaries, seams and test patterns, so many agents can work in parallel without collisions.
3. **Configuration library.** Layered (built-in defaults, system, user, environment and flags), typed, validated, versioned, reloadable. A bad file keeps the old config and reports; it never stops capture.
4. **Transform language.** Choose and specify the mapping language (jq syntax, JMESPath or our own path language) and its development loop: check a mapping against synthetic fixtures, golden tests.
5. **Config-driven profiles.** Mappings per provider API and transport, built-in defaults embedded, a config directory overrides them. Adding or fixing a harness or provider is a config edit, not a rebuild.
6. **Service and install on Linux and macOS.** Background service (launchd, systemd user unit), fixed port, stable CA path, tap file rotation and retention (done: bounded segments and capacity), `soze install` and `uninstall`. Shell functions per harness so plain `claude`, `codex` and `agent` are captured: done in [specs/005-agent-wrappers](../specs/005-agent-wrappers/spec.md).
7. **Reverse-proxy mode.** Harness points its base URL at soze: no CA, and covers plain HTTP to local providers such as Ollama that skip `HTTPS_PROXY`. Needs a per-harness check that the base URL is settable by environment variable.
8. **Dev and test instructions for both platforms.** How to develop and test the service and install on macOS and Linux, a CI matrix for both, and install tests that run against a throwaway home so real shell and service files are never touched.
9. **Run attribution.** Tag each exchange with a run or session id (for example via proxy authentication on the proxy URL) so shared-service captures separate by session.
10. **Emit targets.** How common scheme reaches other applications. Undecided; do not assume a transport, bus or store.
11. **Review fixes.** Write each `unsupported` host and path once per run; keep the error reason in the reply record.
12. **Vocabulary rename.** Move code and specs from "common format", "tool" and per-harness adapter names to the terms above, as files are touched.

13. **Schema fingerprints.** Record a fingerprint of each provider API's wire schema (field names, types, event kinds, never content) and validate live traffic against it, so a harness or provider change is detected and reported instead of silently mis-mapped.
14. **Codex history rebuild.** Per-socket history so every Codex `conversation` and record is the whole conversation, with `partial: true` when the chain breaks and `warmup: true` on the warm-up. Done: [traffic-survey.md](traffic-survey.md) gaps 3 and 6.

## Open questions

- Whether `CODEX_CA_CERTIFICATE` adds to or replaces the system roots.
- Which request header carries a usable version per provider API (Anthropic has an explicit one).
- Which harnesses allow a base URL by environment variable.

## Icebox

- **Hooks.** Executable hook files per profile that receive the mapped common scheme object as their context argument.
