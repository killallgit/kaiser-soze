# Implementation Plan: Agent Traffic Capture

**Branch**: `002-agent-traffic-capture` | **Date**: 2026-10-18 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/002-agent-traffic-capture/spec.md`

## Summary

The proxy already forwards bytes untouched and writes agent-specific tap events (`request` and `response` summaries, stream events) to a JSONL file. This feature changes what those events say. Adapters emit **common-format live events**, and a new agent-neutral `Assembler` folds them into **one finished record per reply**, written when the reply ends, marked incomplete with the dropped count when events were lost or the stream was cut off. Records are written with a waiting send, so a busy output queue cannot lose them; live lines keep the drop-and-count send. The request body is parsed into the common `Conversation` (system, tools, messages). Images become references. No new crate: JSON is built with `serde_json::json!`. Delivery to other applications stays TBD ([docs/objective.md](../../docs/objective.md)); output stays the local JSONL file.

## Technical Context

**Language/Version**: Rust 2021 (existing crate `kaiser-soze`, binary `soze`)

**Primary Dependencies**: existing only (`serde_json`, `tokio`, `hyper`, `bytes`, `flate2`). No new dependency; `serde` derive is not used.

**Storage**: the existing append-only JSONL tap file (mode 0600). Records stay in it; no database or other store.

**Testing**: standard `cargo test`. Unit tests inline in `src/`, proxy integration tests in `tests/proxy.rs`. Fixtures are synthetic.

**Target Platform**: macOS and Linux (local)

**Project Type**: single CLI crate

**Performance Goals**: a live event is in the tap file within 1 s of its chunk reaching the agent (SC-002). Forwarding never waits on the tap (existing).

**Constraints**: forwarding unchanged; bounded queue that drops and counts (existing); request bodies over 32 MiB are recorded as unparsed (existing); message content never printed; credential headers redacted before write.

**Scale/Scope**: two adapters (Codex over WebSocket, Claude Code over SSE); about 6 live event kinds and 1 record kind.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Note |
|-----------|--------|------|
| I. Never in the Way | Pass | Parsing and assembling stay on the tap side of the bounded queue. Assembly of one reply is capped at the request-body bound (32 MiB); past it assembly stops and the record is marked incomplete. The waiting record send blocks only the tap task, never forwarding. A failed parse drops and counts. |
| II. Adapter per Tool | Pass | The `Adapter` and `ExchangeParser` traits keep their shape. Adapters emit common kinds; the Assembler and sink are agent-neutral, so a new adapter needs no core edit (SC-006). |
| III. Tests First | Pass | Each task starts with a failing test. Synthetic fixtures; assertions on kinds, values and structure, never message text. |
| IV. Data Safety | Pass | Image bytes are not stored (FR-004c). Credential redaction is unchanged and precedes writes. Reports print counts and ids. |
| V. Small Code | Pass | One new module, a context struct for assembler inputs. No utility module. |
| VI. Simplify and Delete | Pass | The agent-specific payload shapes and their summary code are deleted once replaced. |
| VII. Self-Documenting Code | Pass | Event kinds and types are named for the common format. |
| VIII. Verify Everything | Pass | The quickstart runs both adapters through the real proxy with synthetic upstreams. |
| Dependencies | Pass | None added. |
| Workflow | Pass | Lane worktree; `task check`; pull request merged after CI. |

Post-design re-check: pass. One risk is recorded below.

## Project Structure

### Documentation (this feature)

```text
specs/002-agent-traffic-capture/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   └── records.md
└── tasks.md             # /speckit-tasks output, not created here
```

### Source Code (repository root)

```text
src/
├── common.rs     # NEW: common-format event kinds and builders; Block, Usage, Conversation as JSON
├── assembler.rs  # NEW: folds live events into the finished record; marks incomplete
├── claude.rs     # EDIT: emit common events; parse the Messages request into a conversation
├── codex.rs      # EDIT: emit common events; parse the Responses request into a conversation
├── exchange.rs   # EDIT: feed live events to the Assembler; write the record at each reply end; count this exchange's output-side drops
├── sink.rs       # EDIT: add a waiting `write_reliable` for records; `write` reports whether it was accepted
├── proxy.rs      # EDIT: write an `unsupported` line when a claimed host's adapter declines a request
├── body.rs       # EDIT: delete `tool_names`; keep `MAX_REQUEST_BODY` as the assembly bound
├── lib.rs        # EDIT: declare the two modules
tests/
└── proxy.rs      # EDIT: assert live feed and finished record through the real proxy
docs/common-format.md   # EDIT: replace "working notes" status with the implemented shapes
README.md               # EDIT: describe the new output lines
```

**Structure Decision**: Keep the single crate. Agent-specific mapping stays in each adapter file. The agent-neutral fold lives in `assembler.rs`.

## Risks

- **Breaking output change**: the JSONL payloads change shape. No consumer exists yet and delivery is TBD, so this is accepted. The README states the new shape.
- **Large request bodies**: parsing a request needs the whole body in memory. The existing 32 MiB cap bounds it; over the cap the exchange is recorded as unparsed and its record is marked incomplete.
- **Slow disk**: if the file writer stalls, a waiting `record` send stalls the tap task, the tap queue fills, and further input is dropped and counted. Forwarding is unaffected.
- **Codex stream shape**: fixtures come from `docs/common-format.md`, not real traffic. Real-agent runs (at most 4 calls, harmless prompt) are the final check and are the user's call.

## Complexity Tracking

No constitution violations to justify.
