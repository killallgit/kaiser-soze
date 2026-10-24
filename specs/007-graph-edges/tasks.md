# Tasks: Graph edges

Order inside a task pair: the test task first, and it must fail before the code task.

## Phase 1: US1, project, session, thread and spawn (P1)

- [X] T001 [US1] Add failing unit tests in `src/claude.rs` for `ClaudeAdapter::identity(&RequestHead) -> Identity` with synthetic heads: a main-agent head (session id, `x-claude-code-request-class: main`) gives `thread == session` and no parent; a subagent head with an agent id gives that agent as the thread and the session as parent; a nested head with `x-claude-code-parent-agent-id` gives that agent as parent; a head with no ids gives an empty identity.
- [X] T002 [P] [US1] Add failing unit tests in `src/codex.rs` for `CodexAdapter::identity`: `session-id` and `thread-id` are read; `x-codex-parent-thread-id` gives the parent; `turn_id` is read from the JSON in `x-codex-turn-metadata`; malformed JSON in that header gives no turn and still gives session and thread.
- [X] T003 [US1] Add `Identity` in new `src/graph.rs` (register `pub mod graph;`), `Adapter::identity` with an empty default in `src/adapter.rs`, and the Claude Code and Codex implementations. T001 and T002 pass.
- [X] T004 [US1] Add failing unit tests in `src/graph.rs` for `edges(exchange, &Identity, Option<&Project>) -> Vec<Value>`: `in_session`, `in_thread` always when the ids exist; `spawned` only with a parent; `in_project` (with the branch) only with a project; node names are `project:`, `session:`, `thread:`, `exchange:`; an empty identity gives no edges.
- [X] T005 [US1] Implement `edges` in `src/graph.rs`. T004 passes.
- [X] T006 [P] [US1] Add failing unit tests in new `src/project.rs` using `tempfile` directories: a plain repository gives its origin remote as `host/owner/repo`; a remote with `user:token@` in it comes out with the credentials removed; no remote gives the resolved path; a linked worktree resolves to the main repository's id and its own branch; a detached `HEAD` gives no branch; a directory in no repository gives `None`; a missing directory gives `None`; the cache returns the stored answer without touching the disk twice.
- [X] T007 [US1] Implement `Project`, `Repository`, `locate` and `ProjectCache` (bounded to 256 directories; caches the repository, re-reads the branch per lookup) in `src/project.rs`. T006 passes.
- [X] T008 [US1] Add a failing integration test in `tests/proxy.rs`: a claimed request carrying identity headers, through a labelled connection whose directory is a temporary repository, writes `in_session`, `in_thread`, `in_project` and (for a parent header) `spawned` edge events; a request without identity headers writes `exchange_start` as today and no edges; the bytes the client receives are identical in both.
- [X] T009 [US1] Wire `begin_exchange` in `src/proxy.rs` to compute the identity, write the edges, and resolve the project on a blocking thread from the label directory. T008 passes.
- [X] T010 [US1] Add failing tests, then implement, that a `record` carries `project`, `session` and `thread` when known and omits them when not (`src/exchange.rs`, `tests/proxy.rs`). Existing record-equals-fold tests still pass.

## Phase 2: US2, continuation and turn (P2)

- [X] T011 [US2] Check, without printing values, that Claude Code's request body has `diagnostics.previous_message_id` and that it equals the previous reply's `message.id`: read the field in a throwaway test fixture shaped from the shape file, then one live run reporting only `equal: true/false`. If false, remove `continues` for Claude Code from the vocabulary and stop this story for that lane.
- [x] T012 [US2] Add failing tests for the step id (`step:<exchange>.<n>`): Claude Code is always `.1`; a Codex socket with three requests gives `.1` to `.3`. Implement in the Codex and Claude parsers.
- [x] T013 [US2] Add failing tests, then implement, `continues` (Claude Code by `previous_message_id`, Codex by completion order on the same thread, as the record is written) with a bounded per-thread state of 1024 threads, and `turn` (Claude Code `x-claude-code-prompt-id`; Codex the header turn for the first step and `client_metadata.turn_id` after).

## Phase 3: US3, tool calls (P2)

- [x] T014 [US3] Add failing tests, then implement, `calls`: a `tool_call` block id seen in a reply and a `tool_result` `call_id` in a later request of the same thread give one edge; an unmatched result or call gives none. Bounded state of 4096 open calls per run.

## Phase 4: Polish

- [ ] T015 [P] Update `specs/002-agent-traffic-capture/contracts/records.md` (the `edge` event, the record ids), `docs/common-format.md` (status), and `README.md`.
- [ ] T016 Run `task check`.
- [ ] T017 One run per lane through the wrappers with `task dev` running: report edge counts by kind, the number of distinct nodes per kind, and that no message text is in the edges. Include one nested-subagent run per lane.

## Dependencies

T001, T002 → T003. T004 → T005. T006 → T007. T003, T005, T007 → T008 → T009 → T010. T011 → T012 → T013. T010 → T014. T014 → T015, T016 → T017.

## Parallel

T002 with T001; T006 with T001–T005 (separate files). T015 alongside T016.
