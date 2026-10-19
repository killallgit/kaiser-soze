# Tasks: Codex system and tools

- [X] T001 [US1] Add failing tests in `src/codex.rs`: developer and system items give `system` entries and leave `messages`; `additional_tools` gives `tools`; top-level `instructions` and `tools` win; a request with none keeps both null.
- [X] T002 [US1] Change `conversation_event` in `src/codex.rs` to fill `system` and `tools` as the plan says, so T001 passes.
- [X] T003 [US1] Run `task check`.
- [X] T004 [US1] Run one live `codex exec` through the proxy and confirm `system` and `tools` are set, reporting shapes and counts only.
- [X] T005 Mark gap 1 fixed in `docs/traffic-survey.md` and correct its description of the cause.
