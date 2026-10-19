# Feature Specification: Codex system and tools in the common scheme

**Status**: Draft
**Input**: Traffic survey run 2 ([docs/traffic-survey.md](../../docs/traffic-survey.md)), gap 1.

## Problem

Codex records always have `system` and `tools` null. The survey's shape file shows why: Codex's WebSocket requests carry neither an `instructions` nor a `tools` field. It sends them inside `input`: system text as `message` items with role `developer`, and tools as one `additional_tools` item whose `tools` array holds namespaces of `function` and `custom` tools. Today the developer text is merged into the first user message and the tools are dropped.

## User Story 1 (P1): A Codex record shows what the model was told and offered

A person reading the tap file sees, for a Codex request, the system text in `system` and the tool list in `tools`, in the same places Claude Code puts them. `messages` holds only user and assistant turns.

**Acceptance**
1. A request with `developer`-role (or `system`-role) message items gives a `system` list of `{type: "text", text}` entries, in order, and no matching message in `messages`.
2. A request with an `additional_tools` item gives `tools` equal to that item's `tools` array, unchanged.
3. A top-level `instructions` string or `tools` array, when present, is still used and takes priority over the in-`input` forms.
4. A request with none of these keeps `system` and `tools` null, as now.
5. Images, tool calls and every other item map as before.

## Requirements

- **FR-001**: Developer and system role message items become `system` entries and never appear in `messages`.
- **FR-002**: An `additional_tools` item becomes `tools`.
- **FR-003**: Top-level `instructions` and `tools` win when present.
- **FR-004**: Tests use synthetic fixtures only and assert on values, not message text.

## Out of scope

Incremental requests (`previous_response_id`), stop-reason vocabulary, the warm-up request. See the survey gaps.
