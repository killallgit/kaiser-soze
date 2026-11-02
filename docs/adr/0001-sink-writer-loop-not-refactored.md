# The Sink writer loop is not refactored

The spawned loop in `Sink::writing` (`src/sink.rs`) looks like a candidate for splitting into smaller parts. We decided to leave it as it is, for three reasons.

1. The stateful concerns are already types: `Rotation` owns the file-system work of closing and deleting segments, and `Tail` owns the last byte written and the file length.
2. The loop is short: 42 lines, `src/sink.rs:138-179`. What is left in it is the order of the steps: stamp the sequence, encode, rotate when due, write, flush.
3. History does not show the loop as a source of defects. Of the 12 commits that touched `src/sink.rs`, two fixed writer-loop behaviour ("Start a new tap line only after a partial one and refuse to open a tap file whose end cannot be read" and "Keep the tap writer running after an I/O error and remove the unused live printer"); the other ten added features or touched other code.

One real gap remains: the stamp-and-encode step has no file-free test. A follow-up adds file-free tests for sequence stamping and for a serialise failure. A test for u64 sequence exhaustion is not planned: the sequence counter is local to the spawned writer, so it is unreachable without a structural seam, and adding one is the refactor this decision declines.
