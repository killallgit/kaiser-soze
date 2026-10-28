# Plan: Graph edges

Rust, no new dependency (the repository metadata is read with `std::fs`; `tempfile` is already a dev dependency). Spec: [spec.md](spec.md). Vocabulary: [docs/common-format.md](../../docs/common-format.md#the-graph). Bead `kaiser-soze-7ug`.

## Files

- `src/graph.rs` (new): `Identity { session, thread, parent_thread, turn }` (each `Option<String>`), `fn edges(exchange, &Identity, Option<&Project>) -> Vec<Value>`, and the `<kind>:<id>` node names. Pure.
- `src/project.rs` (new): `Project { id, branch }`, `Repository` (the stable id and the git dir) and `fn locate(dir: &Path) -> Option<Repository>`: walk up to `.git`; follow a linked worktree's `.git` file to the main repository; read the `origin` url from `config`; strip scheme, user and password. No remote gives the resolved path. Plus `ProjectCache`, a bounded map from directory to `Option<Repository>`; each lookup re-reads the repository's own `HEAD` for the branch, so a checkout shows up.
- `src/adapter/mod.rs`: `Adapter::identity(&self, &RequestHead) -> Identity`, default empty, so a new harness adds a reader and nothing in the core.
- `src/claude.rs`, `src/codex.rs`: implement `identity` from request headers (`x-claude-code-session-id`, `-agent-id`, `-parent-agent-id`, `-request-class`, `-prompt-id`; `session-id`, `thread-id`, `x-codex-parent-thread-id`, and `turn_id` from `x-codex-turn-metadata`).
- `src/proxy.rs`: in `begin_exchange`, compute the identity, write the `edge` events after `exchange_start`, and resolve the project on a blocking thread from the connection label's directory (cached per directory, never awaited by forwarding).
- `src/exchange.rs`: `Recorder` carries the exchange's ids; the `record` payload gains `project`, `session`, `thread` when known.
- Docs: `docs/common-format.md` (drop "proposed" for rows a live run confirms), `specs/002-agent-traffic-capture/contracts/records.md` (the `edge` event and the three record fields), `README.md`.

## Order

1. Identity readers and edge builder (US1), pure, no other change.
2. Project resolution (US1), pure on temp directories.
3. Proxy wiring and the record ids (US1).
4. Steps, `continues` and `turn` (US2), after one body-field check for Claude Code.
5. `calls` (US3).
6. One live run per lane to confirm the edges on real traffic.

## Tests first

Unit tests in `src/graph.rs`, `src/project.rs`, `src/claude.rs`, `src/codex.rs` with synthetic request heads and temporary repositories built by hand (a `.git` folder with a `HEAD` and a `config`). One integration test in `tests/proxy.rs`: a request through the test proxy with the identity headers and a labelled directory writes the edge events and a record with the ids; a request with no identity headers still forwards, writes `exchange_start` as today and no edges.

## Constitution check

Edges come from ids in headers, never message text; tests assert on kinds and node names, not strings. Reading ids and resolving the project run on the tap side, so a failure drops an edge and counts it, and forwarding is untouched. Only a repository's own `HEAD` and `config` are read; credentials in a remote url are removed before anything is written; no credential store is opened and nothing is written under an agent folder. No new dependency.

## Risks found while planning

- A Codex socket carries many requests, so the unit for `continues`, `turn` and `calls` is the step (spec, story 2).
- `continues` for Claude Code needs the request body's `diagnostics.previous_message_id` and the reply's message id, neither of which the tap holds today. Task T016 checks the field before anything depends on it.
