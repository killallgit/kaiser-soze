# Contract: Records in the Tap File

Consumer: any application that reads the JSONL tap file. One JSON object per line. Every line has the envelope in [data-model.md](../data-model.md). Only `event` and `payload` differ.

## Live events (`event` value → `payload`)

| `event` | `payload` | When |
|---------|-----------|------|
| `conversation` | `{system, tools, messages}`, plus `partial: true` and `warmup: true` when they apply (Codex) | The request body is complete |
| `block_start` | `{index, kind, id?, name?}` where kind is `text`, `reasoning` or `tool_call` | A reply block begins |
| `block_delta` | `{index, text?, input_json?, signature?, encrypted?}` | A chunk of a reply block arrives |
| `block_stop` | `{index}` | A reply block ends |
| `usage` | `{input, output, cache_read, cache_write, reasoning}` | The agent reports usage |
| `reply_end` | `{stop_reason}` | The reply ends |
| `error` | `{error_type}` | The upstream reports an error. No message text. |
| `unparsed` | `{side}` where side is `request` or `reply` | The request or reply could not be read (too large, or still compressed) |
| `unsupported` | `{host, path}` | A claimed host's adapter declined to read the request. The bytes were relayed untouched. No content. |

## Finished record

| `event` | `payload` |
|---------|-----------|
| `record` | `{conversation, reply: {message, usage, stop_reason}, incomplete, dropped}` |

## Guarantees

- Within an exchange, lines appear in the order the events occurred.
- A `record` line is written once per reply, when the reply ends. If an exchange ends while a reply is open (a cut-off stream), one more `record` is written for it. A Claude Code exchange has one reply; a Codex WebSocket can have many.
- A `record` line (and the closing `exchange_end` line) is not lost because the output queue is busy. Live lines may be dropped there and counted; the `record` waits for room. The one live line that is refused while its own `record` is being built is not in that record's `dropped` count.
- A `record` equals the fold of its reply's live events. For an incomplete record it equals the fold of the events that were written.
- `incomplete` is true when `dropped` is above 0 (counting live lines lost on the output side), the stream was cut off, the request or reply was unparsed, or assembly stopped at the 32 MiB bound.
- A reply item that is not text, reasoning or a tool call is left out and does not set `incomplete`.
- Image blocks never contain bytes.
- Credential header values never appear in any line.
- Lines of different exchanges may interleave. The `exchange` id separates them.
