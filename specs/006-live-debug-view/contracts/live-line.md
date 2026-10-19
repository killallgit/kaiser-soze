# Contract: live lines

Standard error, one line each, at most 120 columns (cut, never wrapped). Plain text. Fields are separated by two spaces. Time is local `HH:MM:SS`.

```
12:03:41  #3 codex   -> 7 msgs  4 tools  warmup  ~/Code/kaiser-soze
12:03:44  #3 codex   <- completed  2.1s  in 1203 out 88  text,tool_call
12:03:44  #4 claude  unsupported POST /v1/messages/count_tokens
12:03:45  #5 none    tunnel api2.cursor.sh:443  agent ~/Code/kaiser-soze
12:03:46  #3 codex   error rate_limit
```

- Request lines end with the directory, shortened as in [research.md](../research.md); the command name is shown in place of the adapter when a label is present and differs (for example `agent` on a tunnel).
- A `dropped N` live-lines note prints once when the process exits.
- Never present: message, system, tool or reply text, tool names, tool input or output, headers.
- Counts come from array lengths only. Block kinds are `text`, `reasoning`, `tool_call`.
