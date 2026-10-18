# Feature Specification: Agent Traffic Capture

**Feature Branch**: `002-agent-traffic-capture`

**Created**: 2026-10-18

**Status**: Draft

**Input**: User description: "our objective" ([docs/objective.md](../../docs/objective.md))

## Clarifications

### Session 2026-10-18

- Q: In what unit is the common format produced? → A: Both. A live event feed as each chunk arrives, plus one finished record per request and reply when the reply ends.
- Q: When events were dropped or the stream was cut off, is the finished record still produced? → A: Yes. It is always produced, marked incomplete, and carries the dropped count.
- Q: Do image blocks hold the image bytes or a reference? → A: A reference, plus the content type and size. The bytes are not stored.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - See every request and reply without disturbing the agent (Priority: P1)

A person who studies or builds on coding agents runs an agent through the local proxy. Every request the agent sends to its model, and every streamed reply, is captured as it happens. The agent behaves exactly as it does without the proxy.

**Why this priority**: This is the objective itself. Capture is only useful if it is complete, live, and safe for the agent's work.

**Independent Test**: Run a Codex task and a Claude Code task through the proxy with a harmless prompt. Compare the agent's visible result with a run without the proxy. Count captured requests and replies against the agent's own call count.

**Acceptance Scenarios**:

1. **Given** an agent routed through the proxy, **When** it sends a request and receives a streamed reply, **Then** the request and each reply chunk are captured in the order they occurred.
2. **Given** a running capture, **When** the reply streams, **Then** the agent receives each chunk as it arrives and captured data is available before the reply ends.
3. **Given** the capture side is slow or fails, **When** the agent keeps working, **Then** the agent's bytes are not delayed, changed or broken, and the loss is counted.
4. **Given** traffic to a host no supported agent uses, **When** it passes through the proxy, **Then** it is relayed untouched and not captured.

---

### User Story 2 - Receive one common format for every agent (Priority: P1)

An application that consumes agent traffic reads a single stripped-down format. Whether the traffic came from Codex or Claude Code, the consumer sees the same conversation shape: system prompt, tools, messages made of text, reasoning, tool calls, tool results and images, and token usage per reply.

**Why this priority**: The second half of the objective. Without one format, every consumer needs one parser per agent.

**Independent Test**: Capture the same kind of task from each supported agent and confirm both outputs hold the same fields and block kinds, with agent-specific fields left out.

**Acceptance Scenarios**:

1. **Given** a captured Codex exchange and a captured Claude Code exchange, **When** both are converted, **Then** both are expressed with the same conversation, message, block and usage fields from [docs/common-format.md](../../docs/common-format.md).
2. **Given** a reply that streams text, reasoning and a tool call, **When** it is converted, **Then** each appears as its own block in the order the model produced it.
3. **Given** a field only one agent sends, **When** the format does not need it, **Then** it is left out.
4. **Given** a request that carries the whole conversation again, **When** it is converted, **Then** the system prompt and tool list are kept, since only the proxy sees them.
5. **Given** a reply that is still streaming, **When** a consumer reads the live feed, **Then** it sees each event as its chunk arrives, before the finished record exists.
6. **Given** a reply that has ended, **When** a consumer reads the finished record, **Then** its content matches the live events of the same exchange.

---

### User Story 3 - Add a new agent without touching the core (Priority: P2)

A developer adds support for another coding agent by writing one small adapter that names the agent's hosts and turns its traffic into the common format. The proxy core is unchanged.

**Why this priority**: Future-proofing is a stated reason for choosing a proxy. It also keeps each tool's quirks in one place.

**Independent Test**: Add a synthetic adapter in a test and confirm that its traffic is captured and converted with no core file changed.

**Acceptance Scenarios**:

1. **Given** an adapter for a new agent, **When** that agent's traffic passes through the proxy, **Then** it is captured and converted.
2. **Given** the new adapter, **When** the change is reviewed, **Then** no core file was edited.
3. **Given** an agent that sends traffic the proxy cannot read (a private protocol), **When** it runs, **Then** the proxy relays it and reports it as unsupported rather than guessing.

---

### User Story 4 - Keep captured data safe (Priority: P2)

A person who captures traffic can trust that credentials never reach the output and that no agent's own files or trust settings are touched.

**Why this priority**: Captured traffic holds prompts, code and credentials. A leak cannot be undone.

**Independent Test**: Capture traffic that carries credential headers. Search the output for the credential values (count only). Confirm no write under any agent folder and no change to the system trust store.

**Acceptance Scenarios**:

1. **Given** a request with credential headers, **When** it is captured, **Then** the stored copy has those values redacted.
2. **Given** the proxy needs a local certificate, **When** an agent is pointed at it, **Then** the agent trusts it through its own settings and no system trust store changes.
3. **Given** a capture session, **When** it ends, **Then** nothing was written under any agent's own folder and no agent credential file was opened.
4. **Given** any report of a capture run, **When** it is shown, **Then** it gives counts and ids, never message content.

---

### Edge Cases

- A reply stream is cut off halfway: the finished record is still produced from the captured part, marked incomplete.
- A reply grows very large: assembly stops at the bound, and the record is marked incomplete.
- A reply holds an item that is not text, reasoning or a tool call: the item is left out, and later blocks keep their order.
- A request body is very large: it is recorded as unparsed instead of being held in full.
- A reply arrives compressed: it is recorded as unparsed, or decoded when the agent's setup allows it.
- A consumer reads output while the capture is still running.
- The capture falls behind a fast reply: events are dropped and counted, never queued without limit.
- Two agents run through the proxy at once: their exchanges stay separate.
- An agent uses a host the proxy has no adapter for: relayed untouched.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST capture every request an agent sends to its model and every streamed reply, for each supported agent.
- **FR-002**: The system MUST forward each byte to the agent as it arrives. Forwarding MUST NOT wait on capture.
- **FR-003**: A slow or failing capture MUST drop and count events. It MUST NOT delay, alter or break the agent's traffic.
- **FR-004**: The system MUST convert captured traffic into the common format in [docs/common-format.md](../../docs/common-format.md): system prompt, tools, messages, blocks (text, reasoning, tool call, tool result, image) and per-reply usage.
- **FR-004a**: The system MUST produce the common format in two forms: a live event feed, with each event emitted as its chunk arrives, and one finished record per request and reply, produced when the reply ends.
- **FR-004b**: The finished record MUST hold the same content as the live events of the same exchange, in the same order.
- **FR-004c**: An image block MUST hold a reference, the content type and the size. It MUST NOT hold the image bytes.
- **FR-005**: The common format MUST keep only what the model sees and says. Fields a single agent adds MUST be left out unless the format needs them.
- **FR-006**: Support for each agent MUST live in one adapter. Adding an agent MUST NOT require a core change.
- **FR-007**: Traffic to hosts no adapter claims MUST be relayed untouched and not captured.
- **FR-008**: Credential headers MUST be redacted before anything is written.
- **FR-009**: The system MUST NOT install its certificate into a trust store, MUST NOT write under any agent's folder, and MUST NOT open any agent's credential store.
- **FR-010**: Message content MAY be stored in captured output. It MUST NOT appear in terminals, chat logs, test output or commits; reports give counts and ids only.
- **FR-011**: Codex and Claude Code MUST both be supported.
- **FR-012**: Exchanges from concurrent agents MUST stay separate and identifiable.
- **FR-013**: The system MUST count the events it dropped and report the count with each exchange.
- **FR-014**: A finished record MUST be produced for every reply, and for a reply still open when the exchange ends. It MUST NOT be lost because the output is busy. If events were dropped, including live events lost on the output side, or the stream was cut off, it MUST be marked incomplete and MUST carry the dropped count.
- **FR-015**: The memory used to assemble one reply MUST be bounded. When a reply grows past the bound, assembly stops, the record keeps what was assembled, and it is marked incomplete.

### Key Entities

- **Exchange**: One request and its reply. Has an id, the agent it came from, start and end times, and the events within it.
- **Finished record**: The common-format conversation for one exchange, produced when its reply ends.
- **Event**: One captured piece of an exchange, with order and timing.
- **Conversation**: The common-format object a request carries: system prompt, tools, messages.
- **Message**: A role (user or assistant) and an ordered list of blocks.
- **Block**: Text, reasoning, tool call, tool result or image. An image block holds a reference, content type and size, never the bytes.
- **Usage**: Token counts for one reply: input, output, cache read, cache write, reasoning.
- **Adapter**: The per-agent piece that names its hosts and turns its traffic into events.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For a run of each supported agent, 100% of the requests the agent made are present in the capture.
- **SC-002**: A live event is available to a consumer within 1 second of the agent receiving the same chunk.
- **SC-002a**: For 100% of finished exchanges, the finished record matches the live events of the same exchange.
- **SC-003**: An agent task run through the proxy gives the same visible result as without it in 3 of 3 trial runs per agent.
- **SC-004**: When the capture is forced to fail mid-run, the agent still finishes its task, and an incomplete-marked finished record with the dropped count is produced.
- **SC-005**: A consumer reads the output of both supported agents with one parser and 0 agent-specific branches.
- **SC-006**: A new agent adapter is added with 0 changes to core files.
- **SC-007**: A search of the captured output for the values of the credentials used finds 0 matches.
- **SC-008**: After a capture session, 0 files changed under any agent folder and 0 trust-store changes.

## Assumptions

- Users are developers and researchers who run agents locally and consume captured traffic in other applications.
- Capture and conversion happen on the user's machine. Nothing leaves it by default.
- Codex and Claude Code are the supported agents for this feature. Agents whose servers call the model on the agent's behalf (such as Cursor) are out of scope, because the proxy only sees their private protocol.
- How the common format reaches other applications is **TBD** by the user. This feature assumes no transport, message bus, database or store, and delivers the format as local output only.
- The proxy reads traffic that agents send over HTTPS when the agent is set to trust the proxy's local certificate. No agent is modified.
- Existing capture of events already in the repository is the starting point; this feature completes the objective and does not replace it.
