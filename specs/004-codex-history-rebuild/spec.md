# Feature Specification: Codex history rebuild

**Status**: Draft
**Input**: Traffic survey ([docs/traffic-survey.md](../../docs/traffic-survey.md)), gap 3 and 6; roadmap item 14.

## Problem

Codex requests are incremental: each carries `previous_response_id` and only the new `input` items. A Codex record is therefore not the whole conversation, and the common scheme says every record is. A reader that misses the first record of a socket cannot rebuild anything. The first request of each socket is also a warm-up (`generate: false`) with an empty reply that looks like a real turn.

## User Story 1 (P1): Every Codex record is the whole conversation

A person reading the tap file sees, for each Codex request, the full conversation so far: earlier turns, earlier replies, and the new items. `system` and `tools` match what the first request of the socket set.

**Independent test**: feed one parser a first request, its reply, then a request that continues it. The second record holds the first request's messages, the reply message, and the new items, in order.

**Acceptance**
1. A request with no `previous_response_id` starts a new history.
2. A request whose `previous_response_id` is the id of the last reply on the socket is emitted as stored history, then the reply message, then its new items.
3. `system` and `tools` of the first request are carried into later records that have none of their own; a later request that sets them replaces them.
4. A request that continues anything else (proxy joined mid-socket, dropped frame, unknown id) is emitted as sent with `partial: true`. It is never guessed.
5. Once history passes the request buffer bound, later records on that socket are `partial: true`.
6. Two sockets never share history.

## User Story 2 (P2): The warm-up is marked

A reader can skip the warm-up record without guessing.

**Acceptance**
1. A request with `generate: false` gives a record with `warmup: true`. Every other record has no such flag, or false.
2. The warm-up stays in the chain: the next request continues from it.

## User Story 3 (P1, first task): The chain key is confirmed

Before any history code is built, a shape-only check of a live run confirms that `previous_response_id` equals the id in the previous `response.created`. If it does not, chain by socket order instead.

**Acceptance**
1. The result (match or no match) is recorded in the survey doc, with counts and no message content.

## Requirements

- **FR-001**: The Codex parser keeps the running conversation for the life of one WebSocket.
- **FR-002**: Chaining follows the rules in stories 1 and 2; a broken chain is `partial: true`, never repaired by guessing.
- **FR-003**: History is bounded like the request buffer.
- **FR-004**: Claude output and the common record shape for non-Codex are unchanged.
- **FR-005**: Forwarding never waits on this work; a parser failure drops and counts as today.
- **FR-006**: Tests use synthetic fixtures only and assert on values, not message text.

## Out of scope

Stop-reason vocabulary (gap 5), Claude `system` role (gap 4), config-driven profiles.
