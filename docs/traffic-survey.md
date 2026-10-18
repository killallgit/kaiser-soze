# Traffic survey

Shapes only, from one harmless prompt per harness through the proxy (2026-10-18). No content, no ids.
Roadmap item 1; it decides which mappings exist.

## Results

| Harness | Provider and API | Host | Path | Transport | Seen by tap |
|---|---|---|---|---|---|
| `claude` | Anthropic Messages | `api.anthropic.com` | `/v1/messages` | `sse` (POST) | Yes: conversation, block, usage, reply_end |
| `codex exec` | OpenAI Responses (ChatGPT backend) | `chatgpt.com` | `/backend-api/codex/responses` | `websocket` (GET, upgrade, 101) | Yes: two requests on one socket, so two `record` lines |
| `pi` with `strata` (qwen) | Not decrypted | `strata.lan.<private domain>` | unknown | unknown | Tunnel only: host seen, nothing else |
| `agent` (Cursor) | Cursor service | `api2.cursor.sh`, `agentn.global.api5.cursor.sh` | unknown | unknown | Tunnel only: about 20 connections per prompt |

## Findings

- **Most traffic is not model traffic.** One `claude` prompt gave 16 `unsupported` lines, one `codex` run gave 29: bootstrap, plugins, analytics, event logging, MCP registry. The `unsupported` line is correct here, but the volume is noise to filter or fold per run (review fix 3).
- **Versions are in headers.** Anthropic sends `anthropic-version` (and `anthropic-beta`). Codex sends `openai-beta` and a client `version` header. This answers the roadmap question for these two; Cursor and Strata are unknown.
- **Session ids are already in headers.** `claude` sends a session id header; `codex` sends `session-id`, `thread-id` and a window id. Run attribution (item 9) may not need proxy authentication for these two.
- **One socket carries several requests.** `codex` sent two `response.create` frames on one WebSocket, so a mapping must handle a stream of requests per exchange.
- **Unclaimed hosts are invisible.** Strata (plain OpenAI-style, likely Chat Completions) and Cursor (probably binary RPC, unverified) are tunnelled, so their API shape is unknown until the host is claimable by config (items 5, 7).
- **Telemetry hosts appear.** `http-intake` (Datadog), `ab.chatgpt.com`, `registry.npmjs.org` and `mcp-proxy.anthropic.com` are all tunnelled and never claimed.

## Gaps and next steps

- Claim the Strata host through config, then re-run to read its API shape. Needs item 5 or a one-off dev flag.
- Cursor needs its own survey: confirm the protocol before choosing a mapping language that can read it.
- Focus for now: `claude` and `codex`. Only the Strata qwen model works in `pi`; the other harnesses are tested later.
- Not tested: `opencode`, Ollama, API-key (non-ChatGPT) Codex, `api.openai.com` Chat Completions.
