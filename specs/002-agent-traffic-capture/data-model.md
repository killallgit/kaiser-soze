# Data Model: Agent Traffic Capture

All shapes are JSON values. Field lists match [docs/common-format.md](../../docs/common-format.md).

## Envelope

`run_id` (opaque capture epoch), `sequence` (1-based emission order within that epoch), `exchange` (id within that epoch), `adapter` (name), `event` (kind), `mono_us`, `wall_ms`, `start`, `end` (byte offsets), `payload`, and when known `session`, `thread` and (on live event lines) `step`.

Every Proxy instance generates a fresh random `run_id` before emitting any tap lines. All its exchanges and cloned recorders share it, including traffic with no native session or thread. Appending to the same file after a restart preserves the earlier lines and starts a new epoch. See the [envelope contract](contracts/records.md#envelope) for identity scopes and guarantees.

The single writer assigns `sequence` after dequeue, before encoding. `(run_id, sequence)` is a delivery cursor, separate from native identity and capture-attempt identity. Pre-writer drops do not consume numbers; contiguous cursors do not prove complete capture. The [delivery cursor contract](contracts/records.md#delivery-cursor) defines gaps, filtered views and the limits of this metadata.

## Conversation

| Field | Meaning |
|-------|---------|
| system | System prompt as sent, or null |
| tools | List of tool definitions as sent |
| messages | Ordered list of Message |

## Message

| Field | Meaning |
|-------|---------|
| role | `user` or `assistant` |
| blocks | Ordered list of Block |

## Block

| Kind | Fields |
|------|--------|
| text | `text` |
| reasoning | `text`, or `encrypted`; optional `signature` |
| tool_call | `id`, `name`, `input` |
| tool_result | `call_id`, `output`, `is_error` |
| image | `ref`, `content_type`, `size`. Never the bytes. |

A reply holds only text, reasoning and tool_call blocks. tool_result and image blocks appear in requests. A reply item of any other kind is left out of the record; later blocks keep their order. Leaving it out does not make the record incomplete, because the format holds only these kinds.

## Usage

`input`, `output`, `cache_read`, `cache_write`, `reasoning`. Counts, or null when the agent did not send one.

## Finished record

| Field | Rule |
|-------|------|
| conversation | The request's Conversation |
| reply | `message` (assistant Message), `usage`, `stop_reason` |
| incomplete | true if events were dropped (including live events lost on the output side), the stream was cut off, the request was unparsed, the reply was unparsed (still compressed), or assembly stopped at the memory bound |
| dropped | Events dropped in this exchange. 0 when none. |

## State of an exchange

`open` (request seen) → `streaming` (first reply event) → `ended` (reply end marker) or `cut` (stream closed first). A finished record is written at every `ended` reply, and once more on `cut` for a reply still open. `cut` sets `incomplete`. One exchange can hold many replies (a Codex WebSocket), so it can have many records.

## Relationships

- One exchange has one or more replies. Each reply has one Conversation (the request that started it), many live events, and one finished record.
- A finished record equals the fold of the live events of its reply, plus its conversation, which only the record carries.
- Assembly of one reply is bounded by the same size as the request-body cap (32 MiB, `MAX_REQUEST_BODY`).
