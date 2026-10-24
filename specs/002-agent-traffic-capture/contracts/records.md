# Contract: Records in the Tap File

Consumer: any application that reads the JSONL tap file. One JSON object per line. Every line has the envelope in [data-model.md](../data-model.md). Only `event` and `payload` differ.

## Live events (`event` value → `payload`)

| `event` | `payload` | When |
|---------|-----------|------|
| `conversation` | `{system, tools, messages}`, plus `partial: true` and `warmup: true` when they apply (Codex), `previous_message_id` when the request body has `diagnostics.previous_message_id` (Claude Code), and `step` (Codex: 1-based request count on the socket) and `turn` (Codex: `client_metadata.turn_id` when the request has one) | The request body is complete |
| `block_start` | `{index, kind, id?, name?}` where kind is `text`, `reasoning` or `tool_call` | A reply block begins |
| `block_delta` | `{index, text?, input_json?, signature?, encrypted?}` | A chunk of a reply block arrives |
| `block_stop` | `{index}` | A reply block ends |
| `usage` | `{input, output, cache_read, cache_write, reasoning}` | The agent reports usage |
| `reply_end` | `{stop_reason, message_id?}` where `message_id` is the reply's message id when the stream named it (Claude Code `message_start`) | The reply ends |
| `error` | `{error_type}` | The upstream reports an error. No message text. |
| `unparsed` | `{side}` where side is `request` or `reply` | The request or reply could not be read (too large, or still compressed) |
| `unsupported` | `{host, path, label?}` | A claimed host's adapter declined to read the request. The bytes were relayed untouched. No content. |
| `edge` | `{kind, from, to, branch?}` | One relation of the exchange, written after `exchange_start`: `in_project` (session to project, with the branch), `in_session` (thread to session), `in_thread` (exchange to thread), `spawned` (thread to parent thread). Nodes are named `project:<id>`, `session:<id>`, `thread:<id>`, `exchange:<n>`. Only ids from request headers; an edge whose ends are unknown is not written. After each `record` the exchange also writes, per step, `turn` (`step:<exchange>.<n>` to `turn:<id>`) and `continues` (the step to the step before it on the thread: the one whose reply `message_id` equals this request's `previous_message_id`, else the thread's last step). A step whose request carries a `tool_result` that answers an open `tool_call` of the same thread also writes `calls` (`call:<id>` to `result:<id>`); the call then closes, so a result resent in later requests adds nothing, and at most 4096 calls stay open. The thread state is bounded at 1024 threads, dropping the one seen first; a thread keeps at most 1024 reply ids. Without a `previous_message_id` the step before is the last step to finish writing its record, so overlapping requests on one thread are ordered by reply completion, not arrival. A record without a request has no `step` and writes no step edges. A step that names no turn gets the header turn only when it is step 1. |
| `tunnel` | `{host, port, label?}` | A connection to a host no adapter claims was tunnelled, unread. Adapter name `none`. |

`exchange_start` and `tunnel`/`unsupported` carry `label: {command, dir}` only when the connection told the proxy which agent command and directory started it (see the wrappers in the README); it is metadata, not message content.

## Finished record

| `event` | `payload` |
|---------|-----------|
| `record` | `{conversation, reply: {message, usage, stop_reason, message_id?}, incomplete, dropped, step, turn?, project?, session?, thread?}`; `step` is the conversation's step, 1 when it names none (Claude Code); `turn` is the conversation's turn, else the exchange's header turn. A turn id groups replies; it does not say the turn ended |

## Guarantees

- Within an exchange, lines appear in the order the events occurred.
- A `record` line is written once per reply, when the reply ends. If an exchange ends while a reply is open (a cut-off stream), one more `record` is written for it. A Claude Code exchange has one reply; a Codex WebSocket can have many.
- A `record` line (and the closing `exchange_end` line) is not lost because the output queue is busy. Live lines may be dropped there and counted; the `record` waits for room. The one live line that is refused while its own `record` is being built is not in that record's `dropped` count.
- A `record` equals the fold of its reply's live events, plus the `project`, `session` and `thread` ids of its exchange when known. For an incomplete record it equals the fold of the events that were written.
- `incomplete` is true when `dropped` is above 0 (counting live lines lost on the output side), the stream was cut off, the request or reply was unparsed, or assembly stopped at the 32 MiB bound.
- A reply item that is not text, reasoning or a tool call is left out and does not set `incomplete`.
- Image blocks never contain bytes.
- Credential header values never appear in any line.
- Lines of different exchanges may interleave. The `exchange` id separates them.
