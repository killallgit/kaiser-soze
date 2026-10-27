# Contract: live lines

## Current CLI operational and stream output

The CLI uses the bounded metadata logger. `--live` enables request/reply summaries; `--stream` adds per-event metadata in real time and selects `debug` unless a level was explicitly supplied. Both go to stderr. `--log-level off|error|warn|info|debug` and `--log-format human|json` override `SOZE_LOG` and `SOZE_LOG_FORMAT`. Defaults: `info` and human on a terminal, JSON otherwise.

Human records are capped at 160 characters and use UTC time, short run id, exchange id and adapter before other metadata. JSON records keep the full run id and are at most 1024 bytes excluding the newline. Selected fields are numeric counts, indexes, timing/usage values, flags, static field names and safe error categories. Startup may name the CA file. No captured text, reasoning, tool names/inputs/results, headers or credentials are logged at any level.

Stream details are limited to 20 records per one-second window. Summaries and non-error operational events have a separate 20-record budget; errors bypass rate filtering. Every event still uses the bounded 256-item nonblocking queue. Later accepted output carries `suppressed` counts, and queue/oversize loss produces a `log_dropped` event. A failed logging destination never stalls producers; a malformed/oversize diagnostic is dropped and counted without stopping later logging.

Stream observations occur before tap encoding/writing and carry capture identity rather than an emitted delivery cursor. Operational sink failures are reported independently of the failed tap. The logger supplies stderr output; service supervision and retention remain separate concerns.

## Existing library live formatter

The `Live` library formatter remains available for callers that supply their own writer:

Standard error, one line each, at most 120 columns (cut, never wrapped). Plain text. Fields are separated by two spaces. Time is local `HH:MM:SS`.

```
12:03:41  #3 codex  -> 7 msgs  4 tools  warmup  ~/Code/kaiser-soze
12:03:44  #3 codex  <- completed  2.1s  in 1203 out 88  text,tool_call
12:03:44  #4 claude  unsupported /v1/messages/…  claude
12:03:45  #5 none  tunnel api2.cursor.sh:443  agent
12:03:46  #3 codex  error rate_limit
```

- Request lines end with the directory, shortened as in [research.md](../research.md). `unsupported` and `tunnel` lines end with the command name and carry no directory; an unsupported path is cut to its first two parts, because deeper parts can hold account or organization ids.
- When live lines were dropped, the next line that prints is preceded by `(N live lines dropped)`.
- Never present: message, system, tool or reply text, tool names, tool input or output, headers.
- Counts come from array lengths only. Block kinds are `text`, `reasoning`, `tool_call`.
