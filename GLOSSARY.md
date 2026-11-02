# kaiser-soze

A local proxy between a coding agent and its model API. It forwards every byte and taps a copy into events, records and graph edges.

## Language

**Exchange**:
One request an adapter claimed, and its reply; a WebSocket upgrade is one exchange for the whole socket, so an exchange holds one or more steps. A connection can carry many exchanges. Shown at `src/exchange/exchanges.rs:15` and `docs/common-format.md` ("The graph").
_Avoid_: call, connection, request (a request is one step of it)

**Tap**:
The copy of the stream that is read and turned into events while the original bytes go on to the agent untouched; what it writes is the tap file. Shown at `src/exchange/tap.rs:33` and `docs/objective.md` ("Robust").
_Avoid_: log, capture output

**Record**:
One finished request and its reply, written as a single line of the tap file. It is not a live event, which is one line of a stream still in progress. Defined at `src/common/record.rs:6`.
_Avoid_: event, message

**Step**:
The n-th request and reply of an exchange: Claude Code is always step 1, a Codex socket numbers its requests. It is not an exchange. Defined at `src/graph/step.rs:7` and `docs/common-format.md` ("The graph").
_Avoid_: request, round, call

**Turn**:
The steps that answer one user prompt, named by the harness; a warm-up request names none. It is not one model reply. Shown at `src/adapter/codex/mod.rs:15` and `docs/common-format.md` ("The graph", `turn` row).
_Avoid_: prompt, message

**Thread**:
One agent's conversation inside a session: the main agent or a subagent. It is not a session, which a subagent shares with its parent. Defined at `src/graph/mod.rs:28` and `docs/common-format.md` ("The graph").
_Avoid_: session, conversation, agent

**Claim**:
An adapter taking a request, or a host, as its own so the proxy decrypts and parses it. It is not the step a request states, which the code calls `claimed_step`. Defined at `src/adapter/mod.rs:62`.
_Avoid_: match, accept

**Project**:
The git repository a session worked in, named from its origin remote as `host/owner/repo`. It is not a Cargo crate or a working directory. Defined at `src/project/mod.rs:12`.
_Avoid_: repo, workspace

**Edge**:
One typed relation of the graph between two nodes, such as `in_thread` or `continues`. It is not a message link; edges whose far end is unknown are left out. Defined at `src/graph/edge.rs:15` and `src/graph/mod.rs:38`.
_Avoid_: link, relation
