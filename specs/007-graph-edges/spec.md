# Feature Specification: Graph edges

**Status**: Draft
**Input**: User request: map the traffic into a relationship graph across agents and harnesses, and find the edges Claude Code and Codex have in common. The vocabulary is in [docs/common-format.md](../../docs/common-format.md#the-graph). Bead `kaiser-soze-7ug`.

## Problem

The tap holds one conversation per request and one record per reply. Nothing says which requests belong to the same run, which belong to the same agent, or which agent launched which. A subscriber would have to read request headers, know each harness's header names, and match ids itself. Both harnesses already send those ids; soze should read them once and emit edges in one shape for both.

Observed (Claude Code 2.1.302, Codex 0.163.0, one subagent run each; header and key names only):

- Both send a session id, and a subagent shares its parent's session id.
- Codex gives every agent a `thread-id`; a child sends `x-codex-parent-thread-id`.
- Claude Code has no thread id; a subagent sends `x-claude-code-agent-id` and, when it was launched by another subagent, `x-claude-code-parent-agent-id`. The main agent sends neither.
- Neither harness sends a working directory or git field in its headers. Codex's `x-codex-turn-metadata` carries `session_id`, `thread_id`, `parent_thread_id`, `turn_id`, `subagent_kind` and `thread_source`, but no path or repository.

## User Story 1 (P1): Project, session, thread and spawn edges

Each exchange is placed in a project, a session and a thread, and a subagent's thread points at its parent thread. The project is the root: it relates every session, of any harness, that worked on the same git repository.

**Independent test**: feed a synthetic Claude Code request head (main agent) and one with an agent id, and a synthetic Codex head with and without a parent thread id, to the id reader. Each gives the same shape: session, thread, and parent thread when there is one. Nothing is read from message content.

**Acceptance**
1. An `edge` event is written when a request head is read, once per exchange per edge: `in_project` (session to project, when a project is known), `in_session` (thread to session), `in_thread` (exchange to thread) and, when a parent is known, `spawned` (thread to parent thread).
2. Thread id: Codex uses `thread-id`. Claude Code uses `x-claude-code-agent-id` when present, otherwise the session id (the main agent).
3. Parent thread: Codex uses `x-codex-parent-thread-id`. Claude Code uses `x-claude-code-parent-agent-id` when present; a subagent with no such header has the main agent (the session id) as its parent. Both harnesses were observed nesting two levels, so a chain of `spawned` edges is expected.
4. A missing or unreadable header produces no edge for it. The exchange is still forwarded and tapped as today.
5. Nodes are named `<kind>:<id>` (`project:…`, `session:…`, `thread:…`, `exchange:<n>`). Edge events carry no header values other than these ids.

### Project root

The project is the git repository a session worked in, so one view can show every agent and harness on a project and the branches and pull requests each worked on.

6. The project is found from the directory the wrapper's connection label gives (see the live debug view). The proxy walks up from that directory to the nearest `.git`. A linked worktree resolves to the repository it belongs to, so a repository and all its worktrees are one project.
7. The project id is the `origin` remote with the scheme, user, password and port removed and the host lowercased (`host/owner/repo`, path case kept), so the same repository on two machines is one project. A repository with no remote uses its resolved path. A directory in no repository, or no label at all, gives no project and no `in_project` edge.
8. The session also reads the branch name on every exchange, as metadata on the `in_project` edge; a checkout between exchanges shows on the next edge. Matching a branch to a pull request is a subscriber's job; soze does not call GitHub.
9. Only the repository's own metadata is read (`HEAD` and the remote URL in its config). No file contents, no credential stores, and nothing under an agent's folder. Credentials inside a remote URL are removed before anything is written.
10. Finding the project never delays a request: it runs on the tap side, once per exchange (the repository is cached per directory and the branch is read again each time), and a failure costs only the project edge.
    A lookup is skipped, giving no project, when 4 lookups are already in flight.

## User Story 2 (P2): Continuation and turn

A Codex WebSocket is one exchange but carries many requests, each with its own reply. For `continues`, `turn` and `calls` the unit is therefore the **step**: one request and its reply, named `step:<exchange>.<n>` (Claude Code is always `.1`). `in_session`, `in_thread`, `spawned` and `in_project` stay at exchange level because a socket belongs to one thread.

**Independent test**: two synthetic Claude Code exchanges where the second's `diagnostics.previous_message_id` equals the first's reply message id give a `continues` edge from the second to the first. Two Codex exchanges on one thread give `continues` in arrival order.

**Acceptance**
1. `continues` links an exchange to the previous exchange of the same thread. Claude Code uses `diagnostics.previous_message_id` against the earlier reply's message id. Codex uses arrival order on the same thread.
2. `turn` links exchanges that answer one user prompt: Claude Code `x-claude-code-prompt-id`, Codex `turn_id`: the `x-codex-turn-metadata` header gives the first request's turn, and each later request on the socket carries its own in the body's `client_metadata`, which the Codex parser already reads.
3. Before this story is built, one live run confirms that `previous_message_id` equals the earlier reply's message id, and what `root_turn_id` means. A claim that does not hold is removed from the vocabulary, not emitted.

## User Story 3 (P2): Tool call to result

**Independent test**: a synthetic reply with a tool call, followed by a request carrying its result, gives a `calls` edge from the call block to the result block, in both harness shapes.

**Acceptance**
1. `calls` is written when a request carries a `tool_result` whose `call_id` matches a `tool_call` id seen earlier in the same thread.
2. A result with no matching call, or a call whose result never arrives, writes nothing.

## Requirements

- **FR-001**: Reading ids is a pure function of a request head (and, for story 2, a reply id), tested without a socket.
- **FR-002**: Each adapter reads its own ids; the core only writes `edge` events in one shape. A new harness adds an id reader and no core change.
- **FR-003**: Never in the way. Reading ids runs on the tap side. A slow or failing reader drops edges and counts them; it never delays or alters the forwarded bytes.
- **FR-004**: Session, thread and agent ids are not credentials and are written as received. Credential headers stay redacted.
- **FR-005**: Edges are derived from headers and ids only, never from message text.
- **FR-006**: Tests use synthetic fixtures and assert on edge kinds and node ids, never on message strings.
- **FR-007**: A finished `record` carries its `project`, `session` and `thread` ids (each absent when unknown), so a subscriber that reads only records can group them.
- **FR-008**: `docs/common-format.md` and `contracts/records.md` gain the `edge` event once it is written, and unverified rows leave "Status: proposed" only when a live run confirms them.

## Assumptions

- A subagent's session id equals its parent's in both harnesses (observed once each).
- Claude Code's parent for a subagent is the main agent. Observed with one level only.
- The wrapper label is the only source of a directory. Neither harness's headers carry one (checked), so a run started without the wrapper has no project. Claude Code's body `metadata` was not readable (the shape file marks it opaque) and is the one place left unchecked.
- The parent's `Agent` tool result holds the child's agent id (seen in two parent-side records; the field is not identified). It is not needed for `spawned`, which the child's headers give.
- Cached project ids refresh: a directory found to be in no repository is looked up again after 60 s.
- Edges are their own events. The `record` carries the three ids (FR-007) but not the edges.

## Open questions

- Decided: the Claude Code main-agent thread id is the session id, so a session and its main thread share one id.
- Decided: nested subagents work in both harnesses and each child names its parent in a header (observed two levels deep).
- Does a run without the wrapper need a project at all? If yes, the options are a Claude Code body `metadata` field (unchecked) or asking the person to use the wrapper. Out of scope until someone needs it.

## Out of scope

Other harnesses (opencode, pi, Cursor), calling GitHub or any pull request lookup, any emit target or transport (still undecided), storing or querying the graph, and body ids that need the body parsed earlier than today.
