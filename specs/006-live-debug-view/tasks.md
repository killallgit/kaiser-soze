# Tasks: Live debug view

## Phase 1: US1, the lines (P1)

Order inside a task pair: the test task first, and it must fail before the code task.

- [ ] T001 [US1] Add failing unit tests in new `src/live.rs` (register `pub mod live;` in `src/lib.rs`) for `LiveState::line(&mut self, &Value) -> Option<String>` using synthetic tap lines shaped as in `specs/002-agent-traffic-capture/contracts/records.md`: a `conversation` gives one request line with `<n> msgs`, `<n> tools` from array lengths, and `warmup` / `partial` when set; a `record` gives one reply line with stop reason, elapsed seconds from the stored request `wall_ms`, `in <n> out <n>`, block kinds in order (`text,tool_call`), and `incomplete` / `dropped <n>` only when non-zero; `unsupported`, `unparsed`, `error` and `tunnel` each give their line; `block_start`, `block_delta`, `block_stop`, `usage`, `reply_end`, `response_head`, `exchange_start`, `exchange_end` give `None`; `exchange_end` drops that exchange's state; a `record` with no stored request time omits elapsed.
- [ ] T002 [US1] Add failing privacy and shape tests in `src/live.rs`: every fixture string in messages, system, tools, block text, tool names and tool input is distinctive (for example `SECRET-MARKER-…`), and no produced line contains any of them; every line is at most 120 columns, counting characters, with an over-long directory cut from the left with a leading `…`; the directory shows `~` for the home folder (home passed in, not read from the environment); the time is `HH:MM:SS` from `wall_ms` in a passed-in UTC offset of 0 for tests.
- [ ] T003 [US1] Implement `LiveState` and `line` in `src/live.rs` per [contracts/live-line.md](contracts/live-line.md) and [data-model.md](data-model.md). Read only: `event`, `exchange`, `adapter`, `wall_ms`, array lengths of `payload.messages` and `payload.tools`, `payload.partial`, `payload.warmup`, `payload.reply.message.blocks[].kind`, `payload.reply.usage.input` and `output`, `payload.reply.stop_reason`, `payload.incomplete`, `payload.dropped`, `payload.host`, `payload.port`, `payload.path`, `payload.method`, `payload.side`, `payload.error_type`, `payload.label`. T001 and T002 pass.

## Phase 2: US3, switch and printer (P2)

- [ ] T004 [US3] Add failing tests in `src/live.rs` for `Printer`: lines reach a writer in order; with a writer that never accepts, `offer` returns without waiting once the 256-line queue is full and the dropped count rises; `finish` reports the count. Inject the writer (a `tokio::io::AsyncWrite`), so no terminal is used.
- [ ] T005 [US3] Implement `Printer` in `src/live.rs`: bounded queue of 256 lines, `offer(&self, line: String)` using `try_send`, a task that writes to the injected writer, a dropped counter, and a final `dropped N live lines` note on `finish`.
- [ ] T006 [US3] Add a failing integration test in `tests/proxy.rs`: with live output enabled and an injected writer, one request through the test proxy produces a request line and a reply line in the writer and none of the fixture's message text; with it disabled the writer receives nothing; the tap file is identical in both runs apart from timing fields.
- [ ] T007 [US3] Wire it: `ProxyConfig.live: Option<…>` in `src/proxy.rs` and `src/sink.rs` (the writer task runs each record through `LiveState::line` and `Printer::offer`; the tap file write happens first and is never skipped). Add `--live` to `src/main.rs` (stderr writer, home from `HOME`, local UTC offset) and pass `--live` in `scripts/dev.sh` unless `LIVE=0`. T006 passes.
- [ ] T008 [US3] In `src/proxy.rs` print the existing `connect {host}:{port}` line only when live is off.

## Phase 3: US2, agent and directory label (P2)

- [ ] T009 [P] [US2] Add failing unit tests in new `src/label.rs` (register `pub mod label;`) for `from_headers(&HeaderMap) -> Option<Label>` with the rules quoted from [data-model.md](data-model.md): "`command` matches `[A-Za-z0-9_-]{1,32}`; `dir` is UTF-8, under 4096 bytes, starts with `/`, no control characters". Cases: valid `Basic base64(claude:<%XX dir>)`; no header; not `Basic`; bad base64; no colon; empty command; command with a space or 33 characters; directory not starting with `/`; directory with a newline or `\x1b`; directory of 4097 bytes; invalid UTF-8; header present twice. Every failure is `None`, never a panic.
- [ ] T010 [US2] Implement `src/label.rs` with a small base64 decoder and `%XX` decoder (no new dependency) and `Label { command, dir }`. T009 passes.
- [ ] T011 [US2] Add failing tests: in `tests/proxy.rs`, a CONNECT with a valid `Proxy-Authorization` puts `label` on that connection's `exchange_start`, on an `unsupported` event, and on a `tunnel` event for an unclaimed host; a CONNECT with garbage credentials, and one with none, still forward and carry no `label`; an unclaimed host writes a `tunnel {host, port}` event.
- [ ] T012 [US2] In `src/proxy.rs` read the label from the CONNECT request in `open_tunnel`, keep it on `Tunnel`, add it to the `exchange_start` and `unsupported` payloads, and write a `tunnel` event (adapter name `none`) for unclaimed hosts, with the label when present. Never answer 407. T011 passes.
- [ ] T013 [US2] Add failing wrapper tests in `tests/wrappers/test_ready.sh` and `test_fail_open.sh`: when ready, `HTTPS_PROXY` seen by the stub equals `http://<command>:<hex of $PWD as %XX>@127.0.0.1:48731` (compute the expectation in the test with a different tool than the script, for example `printf` in a loop); when the encoder is unavailable (`od` missing from `PATH`) the wrapper uses the plain `http://127.0.0.1:48731`; the not-ready cases are unchanged.
- [ ] T014 [US2] In `scripts/soze-wrappers.sh` add `_soze_label <command>` printing `<command>:<hex>@` (nothing on any failure) built with `printf '%s' "$PWD" | od -An -tx1 | tr -d ' \n' | sed 's/../%&/g'`, and use it in `claude`, `codex` and `agent`. The not-ready path is untouched. T013 passes; update the older ready-case expectations in `test_ready.sh` accordingly.

## Phase 4: Polish

- [ ] T015 [P] Update `specs/002-agent-traffic-capture/contracts/records.md` (the `label` field, the `tunnel` event) and `README.md` (the live view, `LIVE=0`, the label and its limits).
- [ ] T016 Run `task check`.
- [ ] T017 Per agent, one harmless call through the wrapper with `task dev` running: `codex exec --skip-git-repo-check "say hi" </dev/null` (agent run by the assistant), then `claude -p "say hi" </dev/null` and `agent` if installed (assistant runs where possible). For each, record whether the lines carry the command and directory. Put the result in `README.md`. Check that the `task dev` output contains none of the prompt's distinctive words, reporting counts only. The interactive `claude` run and `agent` need a person.

## Dependencies

T001, T002 → T003. T004 → T005. T003, T005 → T006 → T007 → T008. T009 → T010. T010, T007 → T011 → T012. T012 → T013 → T014. T015 independent after T014. T016 → T017.

## Parallel

T009 can be written while T006–T008 are underway (separate file). T015 alongside T016.

## Strategy

MVP is T001–T008: compact content-free lines in `task dev`, no label. The label (T009–T014) follows and can be dropped to a later feature without touching the lines. T017 decides which agents show a directory.
