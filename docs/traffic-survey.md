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

## Run 2: four scenarios per harness (claude, codex)

Scenarios: plain text, one tool call, a second turn, a longer reply. Shapes of the tap output only, so this shows what the current mapping keeps, not the raw wire. Wire fingerprints need raw frames, which the proxy does not store yet (roadmap item 13).

| | `claude` | `codex` |
|---|---|---|
| Model requests per prompt | 1, plus a short side request (no tools, one system entry) | 2 or 3 |
| First request | Real conversation | Empty reply: a warm-up, blocks `[]` |
| `system` | 4 entries | Always null |
| `tools` | 13 on the main request; none on the tool-result follow-up | Always null |
| Block kinds in replies | `text`, `tool_call` | `text`, `reasoning`, one `other` |
| Block kinds in requests | `text`, `tool_result`, `tool_call` | `text`, `reasoning` only |
| Stop reasons | `end_turn`, `tool_use` | `completed` only |
| Usage | `cache_write` set, `reasoning` null | `reasoning` set, `cache_write` null |
| Second turn | Full history resent | Only new items (conversation can be empty or partial) |

## Gaps in the current mapping

1. **Codex `system` and `tools` are never filled.** The request carries `instructions` and `tools`; the common scheme says both exist only on the wire, so this is lost data.
2. **Codex tool calls are not mapped.** The tool scenario produced an `other` block and no `tool_call` or `tool_result`. Stop reason is `completed` even when the model asked for a tool.
3. **Codex requests are incremental.** A later request can carry only new items, so a record's conversation is not the whole conversation. The common scheme says each request carries the whole conversation. This must be decided: rebuild the history per socket (a per-exchange state store) or record the partial request as is.
4. **Claude messages contain a `system` role.** Messages came out as `user,system,assistant,user,system...`. The common scheme allows only `user` and `assistant`. Needs a check of what the extra messages are.
5. **Stop reasons differ.** `end_turn` and `tool_use` against `completed`. The common scheme needs one vocabulary.
6. **Warm-up replies.** Codex's empty first reply is a record with no content. Decide whether to keep, mark or drop it.
