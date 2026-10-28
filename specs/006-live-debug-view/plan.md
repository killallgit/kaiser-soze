# Plan: Live debug view

Rust, no new dependency (base64 and percent decoding are small hand-written functions). Spec: [spec.md](spec.md). Decisions: [research.md](research.md). Types and state: [data-model.md](data-model.md). Formats: [contracts/live-line.md](contracts/live-line.md), [contracts/connection-label.md](contracts/connection-label.md). Validation: [quickstart.md](quickstart.md).

## Files

- `src/live.rs` (new): `LiveState` and `fn line(&mut LiveState, &Value) -> Option<String>`, a pure function of one tap line; plus `Printer`, a bounded queue and a task that writes lines to standard error and counts drops.
- `src/note.rs`: shared `Label { command, dir }` metadata.
- `src/proxy/connect/label.rs`: private `fn from_headers(&HeaderMap) -> Option<Label>`, which reads `Proxy-Authorization`, decodes it and validates it. Any doubt gives `None`.
- `src/sink.rs`: the writer task also offers each record to the `Printer` when one is set (`try_send`, never waits).
- `src/proxy.rs`: read the label from the CONNECT request; keep it on `Tunnel`; add it to `exchange_start`, `unsupported` and a new `tunnel` event; write a `tunnel` event for unclaimed hosts; print the old `connect host:port` line only when `--live` is off.
- `src/main.rs`: `--live` flag. `scripts/dev.sh`: pass `--live` unless `LIVE=0`.
- `scripts/soze-wrappers.sh`: build `http://<command>:<hex of $PWD>@127.0.0.1:<port>`; on any failure to encode, use the plain address.
- `tests/wrappers/`: the ready cases expect the labelled address.
- Docs: `README.md`, `specs/002-agent-traffic-capture/contracts/records.md` (the `label` field, the `tunnel` event).

## Order

1. Formatter and its tests (US1), because it needs no other change.
2. Printer, sink hook, `--live`, dev script (US3).
3. Label parsing and proxy wiring (US2), then the wrapper address.
4. One live check per agent that credentials in the proxy address reach the proxy (`claude`, `codex`, `agent`). An agent that drops them is shown without a label and noted in the README.

## Tests first

Unit tests in `src/live.rs` and `src/proxy/connect/label.rs` with synthetic tap lines. One integration test in `tests/proxy/labels.rs`: a CONNECT with `Proxy-Authorization` puts `label` on `exchange_start`; a CONNECT with garbage credentials still forwards and has no label. Wrapper tests for the labelled address and the plain fallback.

## Constitution check

Message text never reaches the terminal: the formatter reads only counts, block kinds, usage, stop reason, flags, host, path, and the label. A test feeds fixtures whose message, system and tool strings are distinctive and asserts none appears in any line. Label and directory are metadata the person chose, not message content. No new dependency. Forwarding is untouched: the printer queue drops and counts. No credential store is opened, nothing is written under agent folders. Pass before and after design.
