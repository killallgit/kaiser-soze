# kaiser-soze

A local proxy between a coding agent and its model API. It forwards every byte and taps a copy into events, records and graph edges.

## Language

**Exchange**:
One request an adapter claimed, and its reply; a WebSocket upgrade is one exchange for the whole socket, so an exchange holds one or more steps. A connection can carry many exchanges. Shown at `docs/common-format.md` ("The graph") and in `src/proxy/http.rs`, which starts one exchange per claimed request.
_Avoid_: call

**Tap**:
The copy of the stream that is read and turned into events while the original bytes go on to the agent untouched; what it writes is the tap file. Shown at `docs/objective.md` ("Robust").
_Avoid_: log, capture output

**Record**:
One request and its reply, written as a single line of the tap file with its `event` key set to `record`; `incomplete` marks one that lost data: a cut stream, dropped input, an unparsed reply or a truncated one. It is not a live event, which is one line of a stream still in progress. Defined at `src/common/record.rs`.
_Avoid_: message

**Step**:
The n-th request and reply of an exchange: Claude Code is always step 1, a Codex socket numbers its requests. It is not an exchange. Defined at `src/graph/step.rs` and `docs/common-format.md` ("The graph").
_Avoid_: round, call

**Turn**:
The steps that answer one user prompt, named by the harness; a warm-up request names none. It is not one model reply. Shown at `docs/common-format.md` ("The graph").
_Avoid_: message

**Thread**:
One agent's line of work inside a session: the main agent or a subagent. A subagent shares its parent's session, so a session can hold several threads; the main agent's thread has the session's own id. It is not the code's `Conversation`, which is the request payload. Defined at `docs/common-format.md` ("The graph").

**Claim**:
An adapter taking a request, or a host, as its own so the proxy decrypts and parses it. It is not the step a request states, which the code calls `claimed_step`. Defined by `Adapter::claim` and `claims_host` in `src/adapter/mod.rs`.
_Avoid_: match, accept

**Project**:
The git repository a session worked in. Its id is `host/owner/repo` from the origin remote; a repository with no origin remote, or a local-path remote, is named by its path. It is not a Cargo crate. Defined at `src/project/mod.rs` and, for the id, `src/project/id.rs`.
_Avoid_: workspace

**Edge**:
One typed relation of the graph between two nodes, such as `in_thread` or `continues`. An edge whose far end is unknown is left out. Defined at `src/graph/edge.rs` and `src/graph/mod.rs`.
_Avoid_: link
