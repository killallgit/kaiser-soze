# Common format

Status: working notes. Nothing here is implemented beyond the tap events in `src/`.

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

- The model is stateless. Each request carries the whole conversation again.
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
