# Research

## Where the line text comes from
- **Decision**: derive lines from the tap lines already produced, in the sink writer task.
- **Rationale**: one source of truth, so the view can never show something the tap file lacks; the sink task is already off the forwarding path.
- **Alternatives**: print from each parser (couples every adapter to output); print from the proxy at the point of each event (on the forwarding path).

## Not blocking on the terminal
- **Decision**: the sink offers lines to a bounded queue with `try_send`; a printer task writes standard error; a full queue drops and counts.
- **Rationale**: a stalled terminal (paused scrollback, closed pipe) must not slow the tap file or forwarding.
- **Alternatives**: `eprintln!` in the sink task (a blocked terminal stalls the tap file writes).

## How the wrapper tells the proxy the agent and directory
- **Decision**: proxy credentials in the address: `http://<command>:<hex of PWD>@127.0.0.1:<port>`, read from `Proxy-Authorization: Basic` on the CONNECT.
- **Rationale**: the proxy cannot see the agent's working directory, and the only copy in the traffic is inside the system prompt, which is content. CONNECT is the one request a client sends before TLS, and clients that support proxy credentials send them there with no extra configuration.
- **Alternatives**: a custom header (clients cannot add headers to CONNECT); a sidecar file the wrapper writes (races between concurrent agents, needs cleanup); reading the directory from the system prompt (reads content).

## Encoding the directory
- **Decision**: hex of the bytes of `$PWD`, written as `%XX` per byte, produced with `od`, `tr` and `sed`.
- **Rationale**: valid in a URL userinfo for any path, exact for non-ASCII, and available on macOS and Linux. Clients percent-decode userinfo before building the Basic header.
- **Alternatives**: percent-encode only unsafe characters (needs a shell encoder with edge cases); base64 (the proxy would decode twice).

## Validating the label
- **Decision**: command must match `[A-Za-z0-9_-]{1,32}`; directory must decode to valid UTF-8 under 4096 bytes, start with `/`, and contain no control characters. Anything else gives no label.
- **Rationale**: the label is shown in a terminal, so control characters must never reach it, and a bad label must never fail a connection.
- **Alternatives**: strip bad characters (shows a directory the person did not have).

## Showing the directory
- **Decision**: home becomes `~`, then the left is cut with a leading `…` to fit, keeping the last parts.
- **Rationale**: the end of a path says which project it is.
- **Alternatives**: the last part only (two projects with the same folder name look alike).

## Tunnel line for unclaimed hosts
- **Decision**: write a `tunnel {host, port, label?}` event to the tap file for every unclaimed CONNECT, and show it live.
- **Rationale**: without it an agent whose host nobody claims leaves no trace in the tap file or the view.
- **Alternatives**: print only, no tap event (the label is lost for later tools).
