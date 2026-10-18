# Research: Agent Traffic Capture

No `NEEDS CLARIFICATION` remained after `/speckit-clarify`. These are the plan-level decisions.

## Where the mapping lives

- **Decision**: Each adapter emits common-format live events. A new agent-neutral `Assembler` folds them into the finished record.
- **Rationale**: The objective says one small adapter per tool. Keeping the fold in the core means a new adapter only has to emit the common kinds.
- **Alternatives**: Convert in a post-processing step on the JSONL file. Rejected: not live and breaks "fastest". Make each adapter build its own finished record. Rejected: duplicates the fold per agent.

## One file, two forms

- **Decision**: Live events and finished records go to the same JSONL file, told apart by the existing `event` field (`record` for the finished one).
- **Rationale**: Reuses the sink, its bounded queue and its 0600 mode. Delivery is TBD, so adding a second file or channel would assume a transport.
- **Alternatives**: A second file for records. Rejected: two things to rotate and read, no benefit yet.

## Live event kinds

- **Decision**: `conversation`, `block_start`, `block_delta`, `block_stop`, `usage`, `reply_end`, `error`. Details in [contracts/records.md](contracts/records.md).
- **Rationale**: These cover every block kind and the usage in `docs/common-format.md`. Tool results exist only in requests, so they travel inside `conversation`.
- **Alternatives**: One event per block with the whole text. Rejected: not live.

## Finished record content

- **Decision**: `conversation`, `reply` (an assistant message of blocks plus usage and stop reason), `incomplete`, `dropped`.
- **Rationale**: FR-004a, FR-004b, FR-014. The record equals the fold of the exchange's live events (SC-002a).
- **Dropped count**: per exchange, from the existing `Stats.dropped`. Not the global sink counter.

## Incomplete marking

- **Decision**: `incomplete` is true when the exchange had dropped events, the stream ended before its end marker, or the request was recorded as unparsed.
- **Rationale**: FR-014. The record is always written when the exchange ends, including on a client disconnect.

## Images

- **Decision**: An image block holds `ref` (a locator `message index:block index`), `content_type` and `size`. The bytes are dropped during parsing.
- **Rationale**: FR-004c. A content hash would need a hashing crate that is not a direct dependency, so the locator is used.

## JSON without serde derive

- **Decision**: Build values with `serde_json::json!` and read them with `Value` accessors, as the adapters already do.
- **Rationale**: `serde` is not a direct dependency. Adding one needs the user's approval and is not needed.

## Tests

- **Decision**: Unit tests per adapter for each block kind, split chunks and usage. Unit tests for the Assembler (complete, dropped, cut off). One parity test: folding live events equals the finished record. Integration tests drive both adapters through the real proxy with synthetic upstreams.
- **Rationale**: Constitution III. Assert on kinds, indexes, values and flags only.

## Records must not be lost to a busy output

- **Decision**: `Sink` gets a waiting `write_reliable` used only for `record` lines. Live lines keep `write` (try-send, drop and count). The `Recorder` counts, per exchange, the live lines `write` refused; the count is added to the record's `dropped`.
- **Rationale**: FR-014. The waiting send runs in the tap task, not the forwarding path, so it cannot delay the agent. A stalled writer just makes the tap queue overflow, which is already drop-and-count.
- **Alternatives**: Drop records like live lines. Rejected: breaks FR-014. A separate file for records. Rejected: assumes more output plumbing.

## Bound on assembly

- **Decision**: One reply is assembled up to `MAX_REQUEST_BODY` (32 MiB). Past it, assembly stops, the record keeps the assembled part, and `incomplete` is set.
- **Rationale**: FR-015, Principle I. Reuses an existing constant instead of inventing one.

## Reply items outside the format

- **Decision**: An item that is not text, reasoning or a tool call (for example a web-search call) is left out. Later blocks keep their order. `incomplete` is not set.
- **Rationale**: The format keeps only what the model sees and says, and leaves out fields a single tool adds.

## Unparsed replies

- **Decision**: A reply still compressed (`encoded`) is recorded as unparsed and its `record` is marked `incomplete`.
- **Rationale**: The spec edge case; the existing code already notes the encoded state.

## Several replies on one exchange

- **Decision**: The Assembler writes one `record` at every `reply_end`. At exchange end, if a reply is still open, it writes one more `record` marked `incomplete`. The Assembler ignores any event kind that is not a common kind, and writes nothing when it has seen no common event.
- **Rationale**: A Codex WebSocket carries many requests and replies on one exchange id. A Claude Code SSE exchange has exactly one reply, so it gets one record.
