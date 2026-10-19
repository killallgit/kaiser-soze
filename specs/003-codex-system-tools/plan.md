# Plan: Codex system and tools

One file changes: `src/codex.rs`. No new dependency.

- `conversation_event` walks `input` once. Developer and system message items feed a `system` list; an `additional_tools` item feeds `tools`; everything else goes through `input_item` as before.
- Top-level `instructions` and `tools` are used when non-null, so older request shapes keep working.
- Tests first, in `src/codex.rs` tests, with synthetic fixtures.
- Verify once with a live `codex exec` through the proxy: `system` and `tools` set, shapes only.

Constitution check: no content printed, no new dependency, forwarding untouched.
