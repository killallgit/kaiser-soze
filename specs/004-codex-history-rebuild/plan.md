# Plan: Codex history rebuild

File: `src/codex.rs` (parser state and chaining). Docs: `docs/common-format.md` and `docs/traffic-survey.md` (the `partial` and `warmup` fields, the chain-key result). No new dependency. Spec: [spec.md](spec.md). Decisions: [research.md](research.md). State shape: [data-model.md](data-model.md). Validation: [quickstart.md](quickstart.md).

## Approach

- `CodexParser` gains a `History`: the id of the last reply, the stored `system`, `tools` and `messages`, and a byte count against `MAX_REQUEST_BODY`.
- `request_events` builds the request's own conversation as today, then joins it with `History` by the chain rule, and sets `partial` / `warmup` on the payload.
- The reply side watches `response.created` (id) and `response.output_item.done` (finished items). Items map through the existing `input_item`, so the stored reply message has the same shape as a request-side assistant message. `response.completed` closes the reply and commits history.
- No engine change: the record embeds the whole `conversation` payload, so `partial` and `warmup` reach it as they are.
- Order: US3 check first. If ids do not match, chain by socket order and drop the id test from the rules.

## Tests first

Synthetic fixtures in `src/codex.rs` tests: first + continue, new history, system/tools carry and replace, unknown id gives `partial`, over bound gives `partial`, two parsers do not share, warm-up flag and chain through warm-up. 

## Constitution check

No content printed (check uses shapes and counts), no new dependency, forwarding untouched (work is inside the tap parser; failure drops and counts), tests assert values. Pass before and after design.
