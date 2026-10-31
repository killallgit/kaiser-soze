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

## Capture identity and delivery order

Every tap envelope carries an opaque random `run_id` shared by all exchanges of one Proxy instance. A new instance gets a fresh id. At start it closes a non-empty tap file into its own segment, so each closed segment holds one `run_id`. Native conversation identity `(adapter, thread)` stays separate from capture attempt identity `(run_id, exchange, step)` when the step is known; anonymous traffic still has `(run_id, exchange)`. Graph exchange and step names are scoped to their envelope's `run_id`. This is attribution, not a replay, persistence or exactly-once guarantee; see the [envelope contract](../specs/002-agent-traffic-capture/contracts/records.md#envelope).

Every line carries `schema_version`, soze's own output contract version (currently 1); lines from older builds have none. The optional top-level `session`, `thread`, `thread_role` and `step` are additive, so the version stays 1. The single tap writer assigns every emitted line a 1-based `sequence` in file order, forming the delivery cursor `(run_id, sequence)`. Refused inputs consume no sequence values, so contiguous cursors can still carry incomplete capture. Gaps on an unfiltered feed indicate missed emitted output; filtered views may skip values legitimately. Replay, snapshots and durability are separate from this metadata; see the [delivery cursor contract](../specs/002-agent-traffic-capture/contracts/records.md#delivery-cursor).

## The graph

Status is per row below. Observed on Claude Code 2.1.302 and Codex 0.163.0 with one subagent run each. A live run per lane through the wrappers (a plain run and a nested-subagent run) wrote every kind in the table except `follows` and `contains`, with a two-level `spawned` chain in both harnesses and no message text in any edge. The ids below are header names and body key names only; no values are stored here.

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

- Each exchange sets `thread_role` from its own headers, with no state shared across exchanges. Claude Code: `subagent` is `x-claude-code-request-class: subagent` with `x-claude-code-agent-id`; `main` is `request-class: main` with no agent id; anything else, including a subagent class with no agent id and class `main` with an agent id, is `unknown`. Codex: `subagent` is `x-codex-parent-thread-id`, or a non-empty `x-openai-subagent` (observed `collab_spawn`); `main` is `thread-id` equal to `session-id` with neither subagent header; anything else, including a missing id, is `unknown`. `unknown` is never promoted to `main`. Basis: a live probe (probe 2026-10-29) saw 3 main and 1 subagent Claude Code exchanges and 1 main and 1 subagent Codex exchange, so Codex main rests on a one-exchange sample. It is the top-level `thread_role` of every line that carries `thread`.
- The subagent shares its parent's session id in both harnesses, so `in_session` alone does not separate agents. `in_thread` and `spawned` do.
- Claude Code has no thread id. A subagent is told apart by its agent id, and the main agent is the session itself. A subagent request with no agent id has no parent, so it writes no `spawned` edge: its thread would be the session itself.
- A Codex warm-up request (`generate: false`) is a numbered step flagged `warmup: true`. It names no turn, and the `continues` chain runs through it; readers skip it when counting a turn's replies.
- The parent's `Agent` or `spawn_agent` tool call does not carry the child's id. The link is made on the child's side (Codex) or in the parent's tool result (Claude Code).
- The request headers above are already in the tap's `exchange_start` payload with credentials redacted. Body ids are not yet.
