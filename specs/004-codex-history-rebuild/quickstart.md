# Quickstart

1. Chain-key check (US3): `task dev`, run `codex exec --skip-git-repo-check` with a harmless prompt and `</dev/null`, then compare, by count only, the `previous_response_id` of each request with the previous `response.created` id in the tap file. Record match or no match in `docs/traffic-survey.md`.
2. Unit: `task test:unit -- codex::` and `task test:unit -- assembler::`.
3. Gate: `task check`.
4. Live: rerun step 1; each Codex record's `messages` length grows turn by turn, `partial` is absent or false, the warm-up record has `warmup: true`. Report counts only.
