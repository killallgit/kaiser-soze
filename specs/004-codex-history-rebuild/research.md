# Research

## Chain key
- **Decision**: chain on `previous_response_id` equal to the last `response.created` id; confirm live first (US3). Fallback: socket order.
- **Rationale**: it is the field Codex itself uses to chain; an id test catches a dropped frame that order alone would hide.
- **Alternatives**: socket order only (cannot detect gaps); hashing history (costly, no gain).

## Source of the reply message
- **Decision**: collect `response.output_item.done` items and map them with `input_item`.
- **Rationale**: done items are complete, and request-side history already uses the same item shapes, so no second assembler.
- **Alternatives**: run an `Assembler` inside the parser (duplicates engine work); read `response.completed` `output` (not always filled over the socket).

## Where state lives
- **Decision**: in `CodexParser`, one per socket.
- **Rationale**: the socket is the unit the parser already lives for; Claude needs none of this.
- **Alternatives**: engine-level history (couples the engine to one adapter).

## Bound
- **Decision**: stored history counts serialized bytes against `MAX_REQUEST_BODY`; past it, later records are `partial: true` and history stops growing.
- **Rationale**: same bound as the request buffer, no new constant.
