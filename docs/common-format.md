# Common format

Status: implemented. The proxy writes these shapes as live events and finished records; the event kinds and fields are in [specs/002-agent-traffic-capture/contracts/records.md](../specs/002-agent-traffic-capture/contracts/records.md).

## The model seam

Every harness, whatever its storage or wire format, sends the model the same object, and the
model answers with one more message of the same kind:

```
Conversation = system, tools[], messages[]
Message      = role (user | assistant), blocks[]
Block        = text        {text}
             | reasoning   {text | encrypted, signature?}
             | tool_call   {id, name, input}
             | tool_result {call_id, output, is_error}
             | image       {ref}
Usage        = input, output, cache_read, cache_write, reasoning   (per reply)
```

- The model is stateless. Each request carries the whole conversation again. Codex sends only new items after the first request, so its parser rebuilds the whole conversation per socket. A conversation it could not rebuild carries `partial: true`, and a Codex warm-up request (`generate: false`) carries `warmup: true`. Both flags are absent otherwise.
- The harness owns the loop: it runs each `tool_call` itself and appends the `tool_result` as a
  new user message, then calls the model again until it stops.
- A user turn (prompt to final answer) is a harness idea made of several model replies. The API
  knows only requests and replies.
- `system` and `tools` exist only on the wire. No harness stores them on disk, so the proxy is
  the only place to see them.

## What each tool sends

| | Codex 0.163.0 | Claude Code 2.1.302 |
|---|---|---|
| Endpoint | `chatgpt.com/backend-api/codex/responses` | `api.anthropic.com/v1/messages` |
| Transport | WebSocket, `permessage-deflate`, HTTP/1.1 | SSE, HTTP/1.1 |
| Request | Responses API: `instructions`, `tools`, `input[]` items | Messages API: `system`, `tools`, `messages[]` |
| Stream events | `response.created`, `response.output_item.added`, `response.output_text.delta`, `response.function_call_arguments.delta`, `response.reasoning*`, `response.completed` (usage) | `message_start`, `content_block_start`/`delta`/`stop` (`text_delta`, `thinking_delta`, `signature_delta`, `input_json_delta`), `message_delta` (stop reason, usage), `message_stop` |
| Proxy setting | `HTTPS_PROXY` + `CODEX_CA_CERTIFICATE` | `HTTPS_PROXY` + `NODE_EXTRA_CA_CERTS` |

Cursor calls Cursor's own servers (`api2.cursor.sh`, protobuf over HTTP/2) and they call the
model, so a proxy sees Cursor's private protocol, never the model request.

## Mapping the stream to blocks

| Block | Codex | Claude Code |
|---|---|---|
| text | `output_text` deltas of a `message` item | `text` block, `text_delta` |
| reasoning | `reasoning` item, summary deltas or `encrypted_content` | `thinking` block, `thinking_delta` + `signature_delta`; `redacted_thinking` |
| tool_call | `function_call` item, `function_call_arguments.delta` | `tool_use` block, `input_json_delta` |
| tool_result | `function_call_output` input item on the next request | `tool_result` block in the next request's user message |
| usage | `response.completed.usage` | `message_start.usage` + `message_delta.usage` |

A reply ends at `response.completed` (Codex) or `message_stop` (Claude Code).

## The graph

Status is per row below. Observed on Claude Code 2.1.302 and Codex 0.163.0 with one subagent run each. The ids below are header names and body key names only; no values are stored here.

Topology nodes are `project` (a git repository, the root), `session` (one run of a harness), `thread` (one agent's conversation inside a session: the main agent or a subagent) and `exchange` (one model request and its reply). An exchange holds one or more `step`s (`step:<exchange>.<n>`: one request and its reply; Claude Code is always step 1, a Codex socket numbers its requests), and a `turn` groups the steps that answer one user prompt. `call` and `result` nodes (`call:<id>`, `result:<id>`) are the two ends of a tool call. Content nodes are the message blocks above.

| Edge | Status | Meaning | Claude Code | Codex |
|---|---|---|---|---|
| `in_project` | implemented | session belongs to the repository it ran in | from the working directory the wrapper labels the connection with, resolved to the repository's origin remote | same |
| `in_session` | implemented | thread belongs to a session | `x-claude-code-session-id` | `session-id` header, `client_metadata.session_id` |
| `in_thread` | implemented | exchange belongs to a thread | main agent when no `x-claude-code-agent-id`, else that agent id | `thread-id` header, `client_metadata.thread_id` |
| `spawned` | implemented | child thread to the parent thread that launched it | child sends `x-claude-code-request-class: subagent`, `x-claude-code-agent-id`, `x-claude-code-agent-type`, and `x-claude-code-parent-agent-id` when launched by another subagent (observed two levels) | child sends `x-codex-parent-thread-id` and `x-openai-subagent: collab_spawn`; the child has a new `thread-id` and the parent's `session-id`; also `parent_thread_id` in `x-codex-turn-metadata` (observed two levels) |
| `continues` | implemented | step follows the previous step of its thread | `diagnostics.previous_message_id` against the earlier reply's `message.id` (both ids are captured in the tap as `previous_message_id` on `conversation` and `message_id` on `reply_end`; equal in one live run: the second request of a `claude -p --continue` pair named the first reply) | same `thread-id`, ordered by reply completion, not arrival (a socket carries many); an exchange's first record waits for the earlier exchanges of its thread to write their first record or end (at most 2 s), so a held-up record does not let a later step go first |
| `turn` | implemented | exchanges answering one user prompt | `x-claude-code-prompt-id` | `turn_id` in the `x-codex-turn-metadata` header and `client_metadata` |
| `calls` | implemented | `tool_call` to its `tool_result` | `id` to `call_id` | `call_id` |
| `follows`, `contains` | no new data | order of messages, nesting of blocks | by position | by position |

- The subagent shares its parent's session id in both harnesses, so `in_session` alone does not separate agents. `in_thread` and `spawned` do.
- Claude Code has no thread id. A subagent is told apart by its agent id, and the main agent is the session itself. A subagent request with no agent id has no parent, so it writes no `spawned` edge: its thread would be the session itself.
- A Codex warm-up request (`generate: false`) is a numbered step flagged `warmup: true`. It names no turn, and the `continues` chain runs through it; readers skip it when counting a turn's replies.
- The parent's `Agent` or `spawn_agent` tool call does not carry the child's id. The link is made on the child's side (Codex) or in the parent's tool result (Claude Code).
- The request headers above are already in the tap's `exchange_start` payload with credentials redacted. Body ids are not yet.
