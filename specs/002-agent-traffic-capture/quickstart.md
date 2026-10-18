# Quickstart: Validate Agent Traffic Capture

Prerequisites: `task` installed, a clean checkout of the feature branch. Contract: [contracts/records.md](contracts/records.md). Wrap each run in `timeout 300`. Fixtures are synthetic; use the harmless prompt "reply ok" for any real-agent run, at most 4 calls, with `</dev/null`. Report counts and ids only.

## 1. Unit and integration tests (US1, US2)

```sh
task test:unit
task test:integration
```

Expect: pass. The tests cover each block kind, split chunks, usage, dropped events, cut-off streams and the parity check (folded live events equal the finished record).

## 2. Both agents give one shape (US2, SC-005)

Run the integration test that drives the Codex adapter and the Claude Code adapter through the real proxy with synthetic upstreams. Expect: the same `event` kinds and the same `record` fields from both.

## 3. Forwarding is untouched (US1, SC-003, SC-004)

Run the integration test that fills the tap queue or stalls the parser. Expect: the client receives every byte unchanged and in time, the `record` line has `incomplete: true` and a `dropped` count above 0.

## 4. Live before finished (US2/AC5, SC-002)

In the integration test, read the tap file while a synthetic reply is still streaming. Expect: `block_delta` lines are present within 1 s and no `record` line yet. After the reply ends, expect one `record` line.

## 5. Safety (US4, SC-007, SC-008)

Send a request with a synthetic credential header and a synthetic image block. Expect: a search of the tap file for the credential value finds 0 matches, the image block holds `ref`, `content_type` and `size` and no bytes, and no file changed under `~/.claude`, `~/.codex`, `~/.cursor`. Compare directory listings by name only.

## 6. New adapter, no core edit (US3, SC-006)

Run the unit test that registers a synthetic adapter emitting common kinds. Expect: its exchange produces a `record`, and `git diff` for the test shows no change to `src/assembler.rs`, `src/exchange.rs` or `src/proxy.rs`.

## 7. Optional real-agent check

Only with the user's go-ahead: run one Codex task and one Claude Code task through `task dev` with the CA settings from the README. Expect: one `record` per call, `incomplete: false`, and the same call count as the agent made. Delete `target/dev/` afterwards.
