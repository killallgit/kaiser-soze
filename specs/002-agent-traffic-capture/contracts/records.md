# Contract: Records in the Tap File

Consumer: any application that reads the JSONL tap file. One JSON object per line. Every line has the envelope in [data-model.md](../data-model.md). The lines differ in `event` and `payload`, and in the optional `session`, `thread` and `step` below.

## Envelope

Every tap line carries a nonempty string `run_id`, including `tunnel`, `unsupported`, `edge`, `record` and `exchange_end`. It is an opaque random capture identity generated once before a Proxy instance can emit a line and shared by all its exchanges and cloned recorders. The writer uses 128 random bits encoded as hexadecimal; consumers treat the string as opaque. A new Proxy instance gets a fresh identity even when it appends to the same tap file and starts again at exchange 1. Generation uses the existing crypto provider at startup, never the forwarding path.

Every tap line also carries an integer `schema_version`, currently 1: soze's own output contract version, independent of provider wire schemas, the package version and fingerprints. Within a version only additive optional fields appear; removing or renaming a field, or changing a type or meaning, needs a new version. Consumers ignore unknown optional fields and stop on an unsupported version with a compatibility diagnostic, while capture and forwarding continue. Lines from older builds have no `schema_version` and are legacy, never version 1.

Lines written for an identified exchange also carry, at the top level and never `null`, `session` and `thread` (the same ids as the record's `payload.session` and `payload.thread`), and the live event lines carry `step`. A key is left out when its value is unknown, so a tunnel, an unsupported line and an anonymous exchange keep the lines they had. They are additive optional fields, so `schema_version` stays 1.

Identity scopes remain separate:

- Native conversation: `(adapter, thread)`. Session and thread ids are unchanged across capture epochs when the harness sends the same ids.
- Capture attempt: `(run_id, exchange, step)` when the step is known; `(run_id, exchange)` identifies the exchange even for anonymous or unread traffic.
- Graph nodes named `exchange:<n>` and `step:<exchange>.<n>` are scoped to the enclosing line's `run_id`; their bare names are not globally unique.

The capture identity is independent of native harness sessions and wrapper run attribution. Diagnostic `--shape-file` output has its own format and carries none of `run_id`, `schema_version` or `sequence`.

## Delivery cursor

Every emitted tap envelope carries a `sequence`: a strictly increasing, 1-based integer scoped to its `run_id`. The single tap writer assigns it after dequeue, before encoding, so `(run_id, sequence)` describes emission/file order across all exchanges, including reliable writes that waited for queue space. A new proxy run starts at sequence 1 with a fresh `run_id`, even when appending to the same file. No sequence allocation, locks or waits are added to forwarding.

- Sequence describes delivery order, not model causality, producer timestamp order, an observed-event count or speech playback progress.
- Refused inputs before the writer consume no sequence numbers. Capture loss is described by `tap_dropped`, `sink_dropped`, record `dropped` and `incomplete`. Contiguous emitted cursors can coexist with incomplete capture.
- A gap between consecutive cursors of the same run on an unfiltered feed indicates missed emitted output for that consumer. Report the gap; a snapshot API does not yet exist.
- Filtering by adapter, thread or event may legitimately skip global sequence values. A future filtered delivery interface must advance a watermark or signal gaps separately; skipped values in a filtered view alone do not prove loss.
- A cursor supplies ordering metadata, not a replay buffer, snapshot, retention, restart recovery, durable acknowledgement or exactly-once delivery. Queueing and flushing do not imply fsync durability. Future replay must preserve stored cursors rather than re-stamp lines, and unavailable history must be explicit. Another emit target would need the same ordered publication source.

## Live events (`event` value → `payload`)

| `event` | `payload` | When |
|---------|-----------|------|
| `conversation` | `{message_count, tool_count}`, plus `partial: true` and `warmup: true` when they apply (Codex), `previous_message_id` when the request body has `diagnostics.previous_message_id` (Claude Code), and `step` (Codex: 1-based request count on the socket) and `turn` (Codex: `client_metadata.turn_id` when the request has one). The line announces the request by counts and ids; the whole conversation (`system`, `tools`, `messages`) is written once, in the exchange's `record`, so a request is never stored twice | The request body is complete |
| `block_start` | `{index, kind, id?, name?}` where kind is `text`, `reasoning`, `tool_call` or `other` (any block type the adapter does not read) | A reply block begins |
| `block_delta` | `{index, text?, input_json?, signature?, encrypted?}` | A chunk of a reply block arrives |
| `block_stop` | `{index}` | A reply block ends |
| `usage` | `{input, output, cache_read, cache_write, reasoning}` | The agent reports usage |
| `reply_end` | `{stop_reason, message_id?}` where `message_id` is the reply's message id when the stream named it (Claude Code `message_start`; Codex `response.id` on the end frame, else on `response.created` of the same reply) | The reply ends |
| `error` | `{error_type}` | The upstream reports an error. No message text. |
| `unparsed` | `{side}` where side is `request` or `reply` | The request or reply could not be read (too large, or still compressed) |
| `unsupported` | `{host, path, label?}` | A claimed host's adapter declined to read the request. The bytes were relayed untouched. No content. |
| `edge` | `{kind, from, to, branch?}` | One relation of the exchange, written after `exchange_start`: `in_project` (session to project, with the branch), `in_session` (thread to session), `in_thread` (exchange to thread), `spawned` (thread to parent thread; a Claude Code subagent request with no agent id has no parent and writes none). Nodes are named `project:<id>`, `session:<id>`, `thread:<id>`, `exchange:<n>`. Only ids from request headers; an edge whose ends are unknown is not written. After each `record` the exchange also writes, per step, `turn` (`step:<exchange>.<n>` to `turn:<id>`) and `continues` (the step to the step before it on the thread: the one whose reply `message_id` equals this request's `previous_message_id`, else the thread's last step). A step whose request carries a `tool_result` that answers an open `tool_call` of the same thread also writes `calls` (`call:<id>` to `result:<id>`); the call then closes, so a result resent in later requests adds nothing, and at most 4096 calls stay open across the whole run, forgetting the oldest first. A step with no known thread, an empty id, or a call whose reply was never recorded writes no `calls` edge; a result that reaches the tracker before its call's record is written is paired on a later resend. The thread state is bounded at 1024 threads, dropping the one seen first; a thread keeps at most 1024 reply ids. Without a `previous_message_id` the step before is the last step to finish writing its record, so overlapping requests on one thread are ordered by reply completion, not arrival. A record without a request has no `step` and writes no step edges. A step that names no turn gets the header turn only when it is step 1. |
| `tunnel` | `{host, port, label?}` | A connection to a host no adapter claims was tunnelled, unread. Adapter name `none`. |

`exchange_start` and `tunnel`/`unsupported` carry `label: {command, dir}` only when the connection told the proxy which agent command and directory started it (see the wrappers in the README); it is metadata, not message content.

## Finished record

| `event` | `payload` |
|---------|-----------|
| `record` | `{conversation, reply: {message, usage, stop_reason, message_id?}, incomplete, dropped, step, turn?, project?, session?, thread?}`; `step` is the conversation's step, 1 when it names none (Claude Code); `turn` is the conversation's turn, else the exchange's header turn. A turn id groups replies; it does not say the turn ended |

## Guarantees

- Within an exchange identified by `(run_id, exchange)`, lines appear in the order the events occurred.
- A `record` line is written once per reply, when the reply ends. If an exchange ends while a reply is open (a cut-off stream), one more `record` is written for it. A Claude Code exchange has one reply; a Codex WebSocket can have many.
- A `record` line (and the closing `exchange_end` line) is not lost because the output queue is busy. Live lines may be dropped there and counted; the `record` waits for room. The one live line that is refused while its own `record` is being built is not in that record's `dropped` count.
- A live event line (`conversation`, `block_start`, `block_delta`, `block_stop`, `usage`, `reply_end`, `error`, `unparsed`) carries a top-level `step` equal to the `payload.step` of the `record` it is folded into: the step of the reply's conversation, 1 for a request that names none. With no conversation the line and its record have no `step`. A new conversation that cuts an open reply carries the new step; the cut reply and its record keep the old one. Notes, `edge` and `record` lines carry no top-level `step`.
- A `record` equals the fold of its reply's live events, plus its conversation (carried by the record alone, not by the live `conversation` line) and the `project`, `session` and `thread` ids of its exchange when known. For an incomplete record it equals the fold of the events that were written.
- `incomplete` is true when `dropped` is above 0 (counting live lines lost on the output side), the stream was cut off, the request or reply was unparsed, or assembly stopped at the 32 MiB bound.
- A reply item that is not text, reasoning or a tool call is left out and does not set `incomplete`.
- Image blocks never contain bytes.
- Credential header values never appear in any line.
- Lines of different exchanges may interleave. `(run_id, exchange)` separates them, including in a file appended across proxy runs.
