---

description: "Task list for Agent Traffic Capture"
---

# Tasks: Agent Traffic Capture

**Input**: Design documents from `/specs/002-agent-traffic-capture/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/records.md, quickstart.md

**Tests**: Required. The constitution says tests come first: write the failing test, see it fail (red), then write the code (green). Fixtures are synthetic. Tests assert on event kinds, indexes, values and flags, never on message text or error text. Message content is never printed; report counts and ids only. The test framework is the standard `cargo test`.

**Organization**: Tasks are grouped by user story. Run all long commands under `timeout` (300 s default, never above 600 s). Work in the lane worktree. Nothing is committed or pushed until the user asks.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: User story the task belongs to (US1 to US4)

## Phase 1: Setup

- [X] T001 Confirm you are in `.claude/worktrees/spec-objective` on branch `worktree-spec-objective` (`git branch --show-current`), then run `task check` and record the baseline: 32 unit tests and 5 integration tests pass

---

## Phase 2: Foundational (blocks all stories)

**Purpose**: The common-format types and the agent-neutral fold. Both adapters and the exchange loop depend on them.

- [X] T002 Red: add `#[cfg(test)]` tests in new file `src/common.rs` for the event builders. Each builder returns a `TapEvent` whose `kind` is exactly one of `conversation`, `block_start`, `block_delta`, `block_stop`, `usage`, `reply_end`, `error`, with the payload fields in `contracts/records.md`. Cover: `usage` fields `input`, `output`, `cache_read`, `cache_write`, `reasoning` are counts or null; `error` payload holds `error_type` only (no message text); an image block has `ref`, `content_type`, `size` and no other field; a Block `kind` is one of `text`, `reasoning`, `tool_call`, `tool_result`, `image`; a Message `role` is `user` or `assistant`
- [X] T003 Add `pub mod common;` to `src/lib.rs` and implement `src/common.rs` until T002 passes. Build values with `serde_json::json!` (no `serde` derive, no new dependency). Builders with more than 3 arguments take a context struct (for example `BlockStart { index, kind, id, name }`)
- [X] T004 Red: add `#[cfg(test)]` tests in new file `src/assembler.rs` for the fold. Cover: a complete exchange (`conversation`, block events, `usage`, `reply_end`) yields one `record` payload `{conversation, reply: {message, usage, stop_reason}, incomplete, dropped}` with `incomplete` false; `dropped` above 0 sets `incomplete` true and carries the count; finishing without `reply_end` yields one record with `incomplete` true; a request recorded as unparsed yields `incomplete` true; two `reply_end`s on one exchange (WebSocket) yield two records; a reply item that is not text, reasoning or a tool call is skipped and later block indexes keep their order, without setting `incomplete`; a non-common event kind is ignored, and an Assembler that saw no common event writes no record; a reply that grows past `MAX_REQUEST_BODY` (32 MiB, test with a small injected limit) stops assembling, keeps what it has, and sets `incomplete`; an unparsed reply sets `incomplete`; the record equals the fold of the live events (parity)
- [X] T005 Add `pub mod assembler;` to `src/lib.rs` and implement `src/assembler.rs` until T004 passes: `Assembler::push(&TapEvent)` folds events, `Assembler::record(dropped) -> Option<Value>` takes the open reply, `Assembler::finish(dropped) -> Option<Value>` returns the open reply as `incomplete`. `incomplete` is true "if events were dropped (including live events lost on the output side), the stream was cut off, the request was unparsed, the reply was unparsed (still compressed), or assembly stopped at the memory bound". Take the bound as a field with the default `MAX_REQUEST_BODY`

**Checkpoint**: `task test:unit` passes with the new modules.

---

## Phase 3: User Story 1 - See every request and reply without disturbing the agent (Priority: P1) MVP

**Goal**: Live events and one finished `record` per reply reach the tap file, and forwarding stays untouched.

**Independent Test**: [quickstart.md](quickstart.md) sections 3 and 4 (the synthetic-adapter part).

- [X] T006 [US1] Red: add tests in `src/exchange.rs` using a synthetic `ExchangeParser` that emits common-format events. Expect: the live lines appear in order, then one `record` line, then `exchange_end`; a parser stalled until the queue overflows gives a `record` with `incomplete` true and `dropped` above 0; closing the channel before `reply_end` gives one `record` with `incomplete` true. In `src/sink.rs` add a test that a full queue refuses a `write` (and reports it) but a `write_reliable` waits for room and is written; a `record` is present even when live lines were refused, and its `dropped` includes them
- [X] T007 [US1] Edit `run_parser` in `src/exchange.rs` until T006 passes: pass each parser event to `recorder.write` and to the `Assembler`; on `reply_end` write a `record` line with `dropped` read from this exchange's `Stats` (not the global sink counter); when the receiver closes, write the open reply as `incomplete`; keep `exchange_end`. Edit `src/sink.rs`: `write` returns whether the line was accepted and a new `write_reliable(record).await` waits for room. Count, per exchange in `Recorder`, the live lines `write` refused and add that to the record's `dropped`. Use `write_reliable` only for `record` lines
- [X] T008 [US1] Run `task check`. Forwarding code in `src/proxy.rs` and the `TeeBody` in `src/exchange.rs` must be unchanged; confirm with `git diff -- src/proxy.rs` showing nothing and the existing forwarding integration tests still passing. The Assembler ignores non-common kinds and writes no record without a common event, so the existing tests that assert the old tap shape stay green until US2 updates them; `task check` is green at this checkpoint

**Checkpoint**: the exchange loop produces live events and records from any adapter that emits common kinds.

---

## Phase 4: User Story 2 - Receive one common format for every agent (Priority: P1)

**Goal**: Claude Code and Codex both emit the same common events and records.

**Independent Test**: [quickstart.md](quickstart.md) sections 1, 2 and 4.

- [X] T009 [P] [US2] Red: rewrite the tests in `src/claude.rs` for the common shape. Request fixture (split across chunks) gives one `conversation` with `system`, `tools` and `messages`: text, `tool_use` as `tool_call`, `tool_result` with `is_error`, and `image` as an image block holding `ref`, `content_type`, `size`. Stream fixture (split mid-line and mid-JSON) gives `block_start`, `block_delta` (`text`, thinking as reasoning text, `signature`, `input_json`), `block_stop`, one merged `usage` from `message_start` and `message_delta`, `reply_end` with `stop_reason`, and `error` with `error_type` only
- [X] T010 [P] [US2] Red: rewrite the tests in `src/codex.rs` for the common shape. Request (HTTP body and WebSocket text) maps `instructions` to `system`, `tools`, and `input[]` to messages: user and assistant text parts, `function_call` as `tool_call`, `function_call_output` as `tool_result`, reasoning items, and `input_image` as an image block. Stream (SSE and WebSocket) maps `output_item.added` to `block_start`, `output_text.delta` to text `block_delta`, `function_call_arguments.delta` to `input_json`, reasoning deltas to reasoning, and `response.completed` to `usage` plus `reply_end`
- [X] T011 [P] [US2] Edit `src/claude.rs`: emit the common events through `src/common.rs` until T009 passes. A request over the body cap or not parseable stays "recorded as unparsed" and marks the record `incomplete`; a reply still compressed is recorded as unparsed and marks it `incomplete`; leave out reply items that are not text, thinking or tool use
- [X] T012 [P] [US2] Edit `src/codex.rs`: emit the common events through `src/common.rs` until T010 passes, one reply per `response.completed` on a WebSocket; a reply still compressed is recorded as unparsed and marks it `incomplete`; leave out reply items other than message text, reasoning and function calls
- [X] T013 [US2] Delete the dead code left behind: `tool_names` in `src/body.rs` and any old summary payload builders, `DELTA_FIELDS` and unused imports in `src/claude.rs` and `src/codex.rs`. `cargo clippy --all-targets -- -D warnings` passes with no dead-code warning
- [X] T014 [US2] Update `tests/proxy.rs`: the two claimed-stream tests (`relays_a_claimed_sse_stream_live_and_taps_it`, `taps_an_anthropic_stream_through_the_claude_adapter`) assert common kinds and one `record` line. Add a test that drives a Codex-shaped and a Claude-shaped synthetic upstream and asserts both produce the same set of `event` kinds and the same `record` fields (one parser, 0 agent-specific branches). Also assert: N synthetic requests give N `conversation` events and the relayed bytes equal the upstream bytes, repeated in 3 runs (SC-001, SC-003); the agent-specific request fields (`model`, `stream`, `max_tokens`, beta flags) are absent from the `conversation` payload (FR-005); and two exchanges running at the same time have different `exchange` ids with no `record` mixing their blocks (FR-012). The existing test `tunnels_an_unclaimed_host_without_decrypting` covers FR-007
- [X] T015 [US2] Extend the live test in `tests/proxy.rs`: while the synthetic reply is still streaming, read the tap file and assert `block_delta` lines are present within 1 s and no `record` line exists; after the reply ends assert exactly one `record`. Assert the `record` equals the fold of that exchange's live events (parity, SC-002a)

**Checkpoint**: both agents give one shape. `task check` is green.

---

## Phase 5: User Story 3 - Add a new agent without touching the core (Priority: P2)

**Goal**: A new adapter that emits common kinds is captured and recorded with no core edit; unreadable traffic is reported as unsupported.

**Independent Test**: [quickstart.md](quickstart.md) section 6.

- [X] T016 [US3] Add a test in `tests/proxy.rs` that defines a synthetic `Adapter` and `ExchangeParser` emitting only common kinds, registers it in `ProxyConfig`, sends one exchange through the proxy, and asserts the live lines and one `record` appear. The commit for this task edits only `tests/proxy.rs`; check with `git diff --stat` that `src/assembler.rs`, `src/exchange.rs` and `src/proxy.rs` are untouched by it
- [X] T017 [US3] Red then green: for a claimed host whose adapter's `claim` returns `None` (a protocol the adapter cannot read), `src/proxy.rs` currently records nothing. Add a test in `tests/proxy.rs` that expects one `unsupported` event line with `host` and `path` (no content) and the bytes relayed unchanged; then emit that line from `forward` in `src/proxy.rs` using the existing sink and exchange counter. The line holds `host` and `path` without the query string
- [X] T018 [US3] Confirm `contracts/records.md` lists `unsupported` with `{host, path}` exactly as T017 emits it; fix whichever side differs

**Checkpoint**: US1 to US3 work.

---

## Phase 6: User Story 4 - Keep captured data safe (Priority: P2)

**Goal**: No credential value, no image bytes and no content in reports; no agent files touched.

**Independent Test**: [quickstart.md](quickstart.md) section 5.

- [X] T019 [US4] Red then green: add a test in `tests/proxy.rs` that sends a request with a synthetic credential header value and a synthetic image block carrying a unique marker string as its bytes. Assert, by count, that the marker string and the credential value each appear 0 times in the tap file, and that the image block holds `ref`, `content_type` and `size`
- [X] T020 [US4] Audit for leaks: `grep -n "println\|eprintln\|dbg!" src/` shows only the `listening on` and `ca certificate` lines in `src/main.rs`; no code prints a payload. Record the result in the PR description as counts
- [X] T021 [US4] Confirm no new write path: `grep -n "\.claude\|\.codex\|\.cursor" src/` finds no code that writes under agent folders; the tap file and CA dir stay the only write targets

**Checkpoint**: all four stories work.

---

## Phase 7: Polish

- [X] T022 [P] Update `docs/common-format.md`: replace the "working notes, nothing is implemented" status with a short note that the shapes are implemented, and list the event kinds by linking to `specs/002-agent-traffic-capture/contracts/records.md` instead of copying them
- [X] T023 [P] Update `README.md`: the paragraph on JSONL lines describes live events and the `record` line, `incomplete` and `dropped`, and that images are references
- [X] T024 Run `task check`, then quickstart sections 1 to 6 and confirm each expected outcome
- [X] T025 Ask the user before quickstart section 7. Only with a yes: run one Codex task and one Claude Code task through `task dev` (harmless prompt, at most 4 calls per task, `</dev/null`, `timeout`), check one `record` per call with `incomplete` false, report counts and ids only, then delete `target/dev/`
- [ ] T026 When the user asks: commit on the lane branch, rebase onto `main`, run `task check`, open a pull request, merge only after all CI checks pass, then remove the lane worktree

---

## Dependencies and Execution Order

- T001 first. Foundational (T002 to T005) blocks everything: tests before code inside each pair.
- US1 (T006 to T008) needs Foundational. US1 is the MVP.
- US2 (T009 to T015) needs US1. T009 and T010 run in parallel; T011 and T012 run in parallel after their tests; T013 to T015 follow.
- US3 (T016 to T018) needs US1 and US2 (it reuses the common event kinds).
- US4 (T019 to T021) needs US2 (images are parsed there).
- Polish last. T022 and T023 touch different files and run in parallel.

```text
T001 -> T002 -> T003 -> T004 -> T005 -> US1 -> US2 -> US3
                                               US2 -> US4
US1..US4 -> T022,T023 -> T024 -> T025 -> T026
```

## Parallel Opportunities

- T009 (`src/claude.rs` tests) and T010 (`src/codex.rs` tests).
- T011 (`src/claude.rs`) and T012 (`src/codex.rs`).
- T022 (`docs/common-format.md`) and T023 (`README.md`).
- US3 and US4 can run side by side once US2 is done; both edit `tests/proxy.rs`, so their test edits go one at a time.

## Implementation Strategy

1. MVP: T001 to T008. The exchange loop writes live events and records for any adapter that emits common kinds.
2. Move both adapters to the common shape (US2), then US3 and US4.
3. Stop after each checkpoint and run `task check`.
4. The real-agent run (T025) is optional and needs the user's go-ahead.

## Notes

- Total: 26 tasks. Setup 1, Foundational 4, US1 3, US2 7, US3 3, US4 3, Polish 5.
- The JSONL payload shapes change. No consumer exists yet, so this is accepted (see plan Risks).
- No new crate is added. If one looks needed, stop and ask the user.
