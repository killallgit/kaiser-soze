# Feature Specification: Live debug view

**Status**: Draft
**Input**: User request: while `task dev` runs, show one short line per thing that happens (which agent, which directory, whatever is small and important) so a person can see that traffic is arriving, without printing any message content. Resolves the open question about showing objects in the terminal under constitution IV.

## Problem

`task dev` prints nothing once it has said where it is listening. To see anything a person must tail the tap file and write jq. Printing the objects is not allowed: message content never goes to a terminal. The need is a compact, content-free line per event, enough to tell that something is coming in and what kind of thing it is.

## User Story 1 (P1): One compact line per request and per reply

With `task dev` running, a person sees a line when a request arrives and a line when its reply finishes, and nothing else per exchange.

**Independent test**: feed a synthetic tap event sequence (one request, a reply with text and a tool call, a record) to the line formatter; it returns exactly two lines, neither containing any text from the fixtures' messages, system, tool names or tool input.

**Acceptance**
1. A request line shows: time, exchange number, adapter (`claude`, `codex`), message count, tool count, and `warmup` or `partial` when set.
2. A reply line shows: time, exchange number, adapter, stop reason, elapsed time since that exchange's request, input and output token counts, and the block kinds in the reply (for example `text,tool_call`). `incomplete` and the dropped count appear when non-zero.
3. A claimed request the adapter did not read shows an `unsupported` line with method and path, and an unreadable body shows an `unparsed` line with its side. An upstream error shows an `error` line with its code. A connection to a host nobody claims (tunnelled, not read) shows a `tunnel` line with host and port, so an agent the proxy cannot read, such as Cursor's, is still visible.
4. Streaming events (block deltas and the like) print nothing.
5. A line is at most 120 columns. Longer parts are cut, never wrapped.
6. Lines come from fields the tap file already holds. Message text, system text, tool names, tool input and tool output never appear; names a person typed (a directory, a label) are allowed only where story 2 says so.

## User Story 2 (P2): The line says which agent and which directory

The wrapper tells the proxy which command and which working directory started each connection, so lines read `claude ~/Code/kaiser-soze` instead of only an exchange number.

**Independent test**: a client that connects to the proxy with the wrapper's proxy address gets its label and directory in the exchange's lines; a client that connects with a plain address, or with a malformed label, still works and shows no label.

**Acceptance**
1. The wrapper adds the command name and the current directory to the proxy address it sets. The proxy reads them from the connection and attaches them to that connection's exchanges.
2. The directory is shown with the home folder as `~` and cut from the left to fit the line, keeping the last path parts.
3. A missing, empty, malformed or oversized label never fails, delays or alters the connection: the exchange runs as if the label were absent.
4. The label and directory are written to the tap file on `exchange_start` as metadata, so later tools can group by run. They are not message content.
5. An agent that rejects credentials in a proxy address is unaffected in the wrapper's fail-open sense: if it cannot use the labelled address, the label is dropped rather than the proxy (see the assumption below).

## User Story 3 (P2): Turn it on and off

**Acceptance**
1. `soze --live` prints metadata summaries to standard error. `--stream` adds bounded event metadata; operational diagnostics use the same nonblocking logger and severity filter. See the [current CLI output contract](contracts/live-line.md).
2. `task dev` passes `--live` by default and `LIVE=0 task dev` turns it off.
3. Printing is best-effort and never in the way: a slow or closed terminal drops lines and counts them, and never delays forwarding or the tap file.
4. The listening and CA lines at start-up still print first.

## Requirements

- **FR-001**: The formatter is a pure function of one tap line plus a small per-exchange state (request time), so it is tested without a terminal.
- **FR-002**: It reads only fields listed in `contracts/records.md`; it never reads `messages`, `system`, `tools` contents, or block text.
- **FR-003**: Output goes to standard error so jq pipelines on standard output stay clean.
- **FR-004**: Forwarding never waits on the printer; a full print queue drops and counts.
- **FR-005**: Tests use synthetic fixtures and assert on the exact lines for fields (counts, kinds, ids) and on the absence of fixture message text.
- **FR-006**: The connection label is parsed defensively: bounded length, a fixed character set after decoding, and ignored on any doubt.

## Assumptions

- The proxy address format is `http://<command>:<directory>@127.0.0.1:<port>`, sent by Node, Rust and most HTTP clients as a basic proxy credential, with the directory percent-encoded. Whether each agent (Claude Code, Codex, Cursor's `agent`) honours credentials in the proxy address needs one live check each; an agent that does not is shown without a label.
- The model name is not shown: the common scheme deliberately has no `model` field. Showing it would be a separate change to the scheme.
- Directory names are the person's own metadata on their own machine; they are allowed in the terminal and tap file because they are not message content. Message content stays banned.
- Colours, tables and scrolling regions are out of scope: plain lines.

## Out of scope

Run or session ids beyond the label, aggregating or filtering views, a full-screen terminal UI, fixed-port or service work, showing the model name.
