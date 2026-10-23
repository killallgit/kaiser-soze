# Reply, step and user-turn boundaries

Answers kaiser-soze-168.2 (user-turn boundaries for the verbal-kint consumer). Status words: **observed** means the wire carries it and soze writes it; **inferred** means a consumer can guess it from observed data; **unavailable** means no traffic shows it.

## What soze writes

The **tap** is the one file `soze` writes (`tap.jsonl`), one JSON event per line. For each reply it holds two forms of the same data:

- **Live events** (`conversation`, `block_delta`, `reply_end`, and so on) arrive as the reply streams.
- **One `record`** is the common-schema object for that reply, built by folding the live events together. Consumers that want a finished reply read the `record`.

`edge` lines in the same file link records into the graph. There is no second output.

Codex has an internal helper, the **conversation rebuild** (`History` in `src/codex/history.rs`). Codex over WebSocket sends only new items plus a `previous_response_id`, so the parser joins them onto the running conversation to fill `record.conversation` in full. It is not an output. It keeps the last reply's id only to detect a request that does not chain, and it does not write that id to the tap.

## Three identities

| Unit | Meaning | Claude Code | Codex |
|---|---|---|---|
| Reply | one model response | `message_id` on `reply_end` and `record.reply` (observed) | the response id is read by the conversation rebuild but not written to the tap (unavailable today; the fix is a `message_id` on the Codex `reply_end`) |
| Step | one request and its reply | always `step` 1 (observed) | `step` counts requests on the socket, from 1 (observed) |
| User turn | everything answering one prompt | `x-claude-code-prompt-id` header (observed; expected stable across a tool loop, not yet checked live) | `turn_id`: header for the first request, `client_metadata.turn_id` in each later body (observed) |

The tap names a step `step:<exchange>.<n>` in edges. `exchange` restarts at 1 for each proxy run, so step ids are unique only within a run.

## Completion is separate from grouping

A turn id **groups** replies. It never says the turn **ended**. `reply_end` and `record` end a reply, not a turn, so a consumer must not treat either as "turn finished".

| Question | Answer |
|---|---|
| Is this reply the last of its turn? | **Inferred**: the stop reason is not a tool call (`end_turn` for Claude Code, a completed Codex response with no tool-call item). A hook, a subagent or a queued prompt can still follow, so it is a hint. |
| Did the turn end with no final reply? | **Unavailable**: silence is a long tool, a permission wait, or a dead process (168.3). |
| Did the user send a new prompt? | **Observed late**: a new turn id appears on the next request. Nothing shows it before then. |

## Walks

- **Multi-tool turn.** Several steps share one turn id. Each tool-call reply continues the turn; the last non-tool reply is the candidate end. Claude Code gives several exchanges with one `turn` edge target; Codex gives several steps on one socket.
- **Two prompts on one socket (Codex).** Steps 1 and 2 carry different `client_metadata.turn_id`, so they land in different turns. A request with no turn id gets none; it is not assumed to continue the previous turn.
- **Warmup (Codex).** A request with `generate: false` is flagged `warmup` on its conversation and has no real reply. It still takes a step number. Consumers should skip warmup steps when counting a turn's replies.
- **Retry.** Unverified. A retried request would reuse the turn id and produce a second record with a new message id. Both records exist, so a consumer needs a rule for which reply counts; none is chosen yet.
- **Interrupted reply.** The record is written with `incomplete: true` and no `reply_end`, so Claude Code has no `message_id` for it. The next request's `previous_message_id` then matches nothing and gets no `continues` edge. It is not guessed. A Codex cancel frame opens no step.

## What needs another signal

- Turn completion, permission waits and session end: a harness hook or wrapper signal (168.3).
- A stable key for resume and for "newest turn": exchange ids restart per run, so catch-up must key on thread plus turn plus step with a time order, or on a message id for Claude Code. Choosing it belongs to the delivery contract (168.4).
- The Codex reply id: write it as `message_id` on the Codex `reply_end`, as Claude Code already does.

## Catch-up without skips or duplicates

Latest catch-up is safe only if each reply has a key that survives a proxy restart. Claude Code has one (`message_id`). Codex does not yet. Until it does, a consumer joining after a restart can show a Codex reply twice or miss one; this is the open point for 168.4.
