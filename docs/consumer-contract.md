# Consumer contract proposal (verbal-kint)

Status: **proposal, not approved.** It answers the exploration beads kaiser-soze-168.1 (conversation identity), 168.3 (waiting, activity, session end), 168.4 (catch-up, live, recovery) and 168.5 (assistant-text filtering), under the epic kaiser-soze-168 (soze consumer contract). Reply, step and turn identities (168.2) are in [turn-boundaries.md](turn-boundaries.md). It chooses no transport, store or vk migration. Evidence words: **observed** (the wire or tap carries it), **derived** (computed from observed data), **stale** (was observed, may no longer hold), **unknown** (no traffic shows it).

## What vk expects

vk owns the `Source` seam in `src/source.rs`. A source lists `Conversation { id, title?, workspace?, state, process? }`, sends a catalog `Snapshot` of `Listed { conversation, replaces? }`, and sends per-conversation `Event`s: `Turn { turn: { id, blocks: [Text] }, finished }`, `NewConversation`, `Ended`, `Warning`, `Failed`. `StartAt::Latest` and `Newest` pick where playback joins. vk plays what the source sends; "finished" is the source's call. vk assumes a turn's text only grows by whole blocks.

## Field mapping

| vk field | From soze | Evidence |
|---|---|---|
| `Conversation.id` | the scope `(adapter, thread id)` | observed |
| `title` | nothing in traffic | unknown |
| `workspace` | `label.dir` from the wrapper, else the project id and branch | observed when wrapped, else unknown |
| `state` | see [Lifecycle](#lifecycle-168-3) | mostly unknown |
| `process` | nothing in traffic (a wrapper could report the child pid) | unknown |
| `Turn.id` | the turn id | observed |
| `Turn.blocks` | `text` blocks of the turn's main-thread replies | derived |
| `finished` | no authoritative source | derived at best |
| `replaces` | nothing in traffic | unknown |

## Identity and subagent selection (168.1)

Soze keeps `project`, `session` and `thread` distinct and leaves interpretation of native ids in the adapters.

- **Scope.** A conversation is `(adapter, thread id)`. Native ids come from the harness, so they survive a soze restart; exchange ids do not. Two harnesses cannot collide because the adapter name is part of the key. Edge node ids (`thread:<id>`) carry no adapter, so the key is built from the `adapter` field of the tap line.
- **Main thread.** Claude Code's main agent sends no agent id, so its thread is the session id. A Codex main thread has no parent header. Both are defined by the *absence* of a header, and a missing header is also what an unidentified request looks like. The adapter reads only `request-class: subagent`; any other value, and the set of classes Claude Code sends, is unverified. A `subagent` request that carries a session id but no agent id would get thread = parent = session, a `spawned` self-edge that no test covers.
- **Proposal.** Add an explicit `role` per thread, set by the adapter only from positive evidence: `subagent` (Claude `request-class: subagent` is read by the code; Codex `x-openai-subagent` is in docs/common-format.md from Codex 0.163.0 but the code reads only `x-codex-parent-thread-id`), `main` (needs a header or value proven live to mean main; only a synthetic test uses `request-class: main` today), else `unknown`. vk lists and plays `main` only. `unknown` threads are never promoted to main.
- **Catalog.** `title` is unknown, so vk shows `workspace` (directory, else project id and branch) plus start time, and soze offers no title. Worktrees of one repository share a project id and differ in directory and branch, so they stay separate conversations.
- **Late or missing edges.** Records already carry `session` and `thread`, so the thread is known at once; the edges are written after `exchange_start`, so the `role` would arrive after it. Until the role is known the thread is `unknown`, not main.
- **Replacement (`/clear`).** A new session id in the same window is a new conversation. Soze cannot see the window, so `replaces` stays unset. A wrapper could link them (unverified).
- **Restart.** Conversations come back by native id on their next request; soze cannot list idle sessions it has not seen traffic from, so the catalog is *observed traffic*, not a complete list of live sessions.

Walk: a main thread with two nested subagents gives three threads in one session; `spawned` edges link them; vk plays only the `main` role.

## Lifecycle (168.3)

| State | Claude Code | Codex |
|---|---|---|
| Reply streaming | observed (stream open) | observed |
| Tool running or permission wait | **unknown**: both look like a tool-call reply followed by silence | unknown, same |
| Turn likely over | derived: a non-tool stop and no new request | derived: completed response with no tool call |
| Session exited | unknown from traffic | unknown |
| Capture loss | observed: `incomplete` and `dropped` on the record, and `tap_dropped` and `sink_dropped` on `exchange_end`. `dropped` is per exchange, so on a Codex socket it is cumulative across replies | same |
| Soze or consumer disconnected | consumer side | consumer side |

- **Permission wait vs long tool.** Traffic cannot separate them. Waiting-for-user needs a harness signal.
- **Options.** (a) A supplemental signal path: a wrapper sends child-exit, and later a harness hook sends waiting/activity events into the same stream. (b) Keep vk's existing polling as transitional enrichment. That is the `claude agents --json` command, run every 1 s while busy and every 5 s otherwise, mapping `status` and `waitingFor` to `State` (an unknown status becomes Idle). It exists for Claude only, so Codex has nothing to keep. Recommendation: (b) now, plus the wrapper exit signal, and design (a) for hooks later.
- **Reaching vk.** Traffic-derived activity should arrive as events (`activity: streaming | tool_pending | quiet`), not only as snapshots, with `stale` after a quiet interval. vk's `State` would gain an `Unknown` variant, and the phone list would need a way to show "unknown" instead of guessing idle.

## Delivery contract (168.4)

Required behaviors, independent of transport:

| Behavior | Requirement | Gap today |
|---|---|---|
| Identity across restarts | native conversation keyed by `(adapter, thread)`; capture attempts by `(run_id, exchange, step)` when the step is known | `run_id` implemented on every tap envelope; native identity is unchanged |
| Ordering | emission/file order across exchanges, separate from model causality and producer timestamps; pre-writer drops use loss counters | writer-assigned 1-based `sequence` implemented on every tap envelope; it restarts with a new `run_id` |
| Schema compatibility | every tap envelope carries a schema version | none written |
| Snapshot to live | a snapshot carries the cursor it was taken at; live resumes from that cursor | cursor metadata exists; no snapshot/subscription API or atomic handoff |
| Resume | a future consumer resumes from `(run_id, sequence)` with explicit history-unavailable / resync-required when recovery is unavailable | cursor metadata exists; no replay buffer, resume protocol or restart recovery |
| Duplicates and gaps | delivery duplicates identified by cursor; an unfiltered same-run sequence gap reports missed emitted output | no recovery API; semantic reply deduplication still lacks Codex provider reply ids |
| Filtered feeds | advance a watermark or signal gaps separately; skipped global values alone do not prove loss | no filtered delivery interface |
| Durability | state the persistence and acknowledgement guarantees explicitly | queued or flushed lines do not imply fsync durability or durable acknowledgement |
| Retention | bounded history; a consumer asking for dropped history gets a snapshot and a marker | rotation and retention are roadmap item 6 |
| Resource use | bounded queues; a slow consumer is dropped or lagged, never waited on | already true for the tap |

Plausible emit options, none approved: (1) tail the tap file with a cursor; (2) a local socket stream with replay; (3) a local HTTP stream; (4) a library the consumer embeds. Against the table: (1) is simplest but pushes rotation, cursors and snapshots onto every consumer; (2) and (3) can own cursors and handoff in the service; (4) couples consumer and proxy lifetimes. All must keep "never in the way".

The implemented cursor `(run_id, sequence)` describes emitted output only. Pre-writer rejections consume no sequence numbers; `tap_dropped`, `sink_dropped`, record `dropped` and `incomplete` describe capture loss. A contiguous sequence does not prove complete capture. Sequence is not an observed-event count, native conversation identity or speech bookmark. Future replay must preserve stored cursors, and another emit target must share the ordered publication source. Today a consumer can report a gap; it cannot request a fresh snapshot.

Target recovery walks, still requiring a delivery API: *join after answer* takes a snapshot then follows from its cursor; *update during handoff* is covered by that cursor; *slow consumer* is lagged and told to resync; *reconnect* resumes from its cursor if history is available; *service restart* changes the run id and starts sequence 1, requiring an explicit recovery decision; *partial record* (`incomplete: true`) is delivered flagged, never as finished; *unavailable history* returns history-unavailable / resync-required, with a snapshot only if a future API provides one.

## Assistant text (168.5)

- **Eligible prose.** `text` blocks of assistant replies on a `main` thread. Excluded: `reasoning`, `tool_call`, `tool_result`, `image`, and every subagent thread by default. No commentary/final marker is observed in either harness, so soze offers none.
- **Committed blocks vs deltas.** The record is a committed whole reply; live `block_delta` events are incremental, with block indexes local to one reply. vk assumes whole append-only blocks, so the smallest compatible change is to deliver committed blocks only: a text block is eligible once its `block_stop` arrives. The Claude `message_id` is read at `message_start` but written only on `reply_end`, after the deltas, so a live consumer cannot key deltas by it. Live text pairs with its record by `(run_id, exchange, step, index)`; `(message_id, index)` is a stable key only once the reply has ended, and Codex has no reply id yet.
- **Traces** (synthetic): live text then its record, where the record replaces the live copy by `(run_id, exchange, step, index)`; an extended block, where a later delta of the same key appends; duplicated delivery, dropped by key (Claude Code only, and not for an interrupted reply, which has no `message_id`); a missing delta, detected by the `dropped` count and `incomplete`, so the block is not played as final; a correction, which needs revision semantics soze does not have; several replies on one Codex socket, separated by `step`.
- **Belongs in vk:** prose eligibility, speech-specific text cleanup, and the main-only default. **Must survive normalization:** thread, role, turn, step, reply id, block index, run id, and the `incomplete` flag.
- **Smallest vk change.** None to the `Source` trait. A soze `Source` maps a turn's committed text blocks to `Event::Turn` with `finished` set by a stated rule (derived, see Lifecycle); `Newest` maps to the newest closed turn.

## Soze work this implies

1. Codex reply id as `message_id` on `reply_end` (from the turn boundaries doc).
2. A thread `role`, from positive evidence only, with a live check of Claude request classes.
3. A run id and writer-order line sequence number (implemented on every tap envelope), plus a schema version.
4. A wrapper child-exit signal.

Each is a separate change; none is started by this document.
