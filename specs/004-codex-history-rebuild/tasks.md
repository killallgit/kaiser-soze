# Tasks: Codex history rebuild

## US3 (first): confirm the chain key

- [ ] T001 [US3] Run one live `codex exec --skip-git-repo-check` through `task dev` (harmless prompt, `</dev/null`, at most 4 calls). From `target/dev/shapes.jsonl` and the tap file, count requests whose `previous_response_id` equals the previous `response.created` id. Report counts only, never content or ids.
- [ ] T002 [US3] Record match or no match in `docs/traffic-survey.md` gap 3. On no match, change the chain rule in `spec.md`, `plan.md` and `data-model.md` to socket order before T003.

## US1: whole conversation per record (P1)

- [ ] T003 [US1] Add failing tests in `src/codex.rs` (synthetic fixtures, assert values not text): continue gives stored messages + reply message + new items; no `previous_response_id` resets history; `system`/`tools` carry forward and a later request that sets them replaces them; unknown id gives `partial: true` and leaves history unchanged; over `MAX_REQUEST_BODY` gives `partial: true`; two parsers share nothing.
- [ ] T004 [US1] Add a failing test in `src/assembler.rs`: `partial` and `warmup` on a `conversation` payload appear on the record.
- [ ] T005 [US1] Add `History` (fields per `data-model.md`) to `CodexParser` in `src/codex.rs`. Track `response.created` id and `response.output_item.done` items (mapped with `input_item`); commit on `response.completed`. Join in `request_events` by the rules, so T003 passes.
- [ ] T006 [US1] Copy `partial` and `warmup` from the conversation payload into the record in `src/assembler.rs`, so T004 passes.

## US2: mark the warm-up (P2)

- [ ] T007 [US2] Add failing tests in `src/codex.rs`: `generate: false` gives `warmup: true`; other requests do not; the next request continues from the warm-up.
- [ ] T008 [US2] Set `warmup` in `request_events` in `src/codex.rs`, so T007 passes.

## Polish

- [ ] T009 Run `task check`.
- [ ] T010 Run one live `codex exec` through the proxy; confirm `messages` grows per turn, warm-up has `warmup: true`, `partial` absent. Report counts only.
- [ ] T011 Update `docs/common-format.md` (`partial`, `warmup`) and mark gaps 3 and 6 fixed in `docs/traffic-survey.md` and roadmap item 14 in `docs/roadmap.md`.

## Dependencies

T001 → T002 → T003..T008. T003/T004 are tests for T005/T006 and can be written together; T007/T008 follow T005 (same file). T009 → T010 → T011.

## Strategy

MVP is T001–T006: whole conversations. Warm-up marking follows.
