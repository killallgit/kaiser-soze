# Data model

**Label** `{command, dir}`: `command` matches `[A-Za-z0-9_-]{1,32}`; `dir` is UTF-8, under 4096 bytes, starts with `/`, no control characters. Absent when the connection carried none or anything failed validation.

**LiveState** (inside the formatter): per exchange number, the label and the wall time of its last `conversation`. Entries are removed on `exchange_end`. A `record` with no stored request time shows no elapsed time.

**Printer**: a queue of 256 lines, a task that writes standard error, a counter of dropped lines. On exit it prints the drop count when non-zero.

**Tap file additions** (all optional fields, nothing renamed): `label` on `exchange_start`, `unsupported` and the new `tunnel` event; `tunnel` payload `{host, port, label?}`.

**Line fields**:
- request: time, `#<exchange>`, adapter, `->`, `<n> msgs`, `<n> tools`, `warmup`/`partial` when set, directory
- reply: time, `#<exchange>`, adapter, `<-`, stop reason, elapsed seconds, `in <n> out <n>`, block kinds, `incomplete`/`dropped <n>` when set
- other: `unsupported <METHOD> <path>`, `unparsed <side>`, `error <code>`, `tunnel <host>:<port>`
