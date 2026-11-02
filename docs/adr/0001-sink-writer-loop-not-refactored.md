# The Sink writer loop is not refactored

The spawned loop in `Sink::writing` (`src/sink.rs`) looks like a candidate for splitting into smaller parts. We decided to leave it as it is, for three reasons.

1. The stateful concerns already have their own types: `Rotation` owns the segment state and the file-system work of closing and deleting segments, and `Tail` owns the last byte written and the file length. The loop reads `Tail` and resets the rotation's limit after a failed rotation.
2. The loop is short: 42 lines, `src/sink.rs:138-179`. What is left in it is the order of the steps: have the caller's `prepare` stamp the sequence, encode, rotate when due, repair a partial last line, write, flush.
3. History does not show the loop as a source of defects. Of the 12 commits that touched `src/sink.rs`, two fixed writer-loop behaviour ("Start a new tap line only after a partial one and refuse to open a tap file whose end cannot be read" and "Keep the tap writer running after an I/O error and remove the unused live printer"); the other ten added features or touched other code.

The stamp-and-encode step is covered by file-free tests: `items_reach_prepare_and_the_writer_with_sequences_one_two_three_in_arrival_order` for sequence stamping and `an_item_that_cannot_be_serialised_consumes_no_sequence_number` for a serialise failure. A test for u64 sequence exhaustion is not planned: the sequence counter is local to the spawned writer, so it is unreachable without a structural seam, and adding one is the refactor this decision declines.
