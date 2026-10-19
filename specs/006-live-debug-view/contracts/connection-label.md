# Contract: connection label

Sent by the wrapper, read by the proxy on the CONNECT request.

- Proxy address: `http://<command>:<dir>@127.0.0.1:<port>`.
- `<command>`: `claude`, `codex` or `agent`.
- `<dir>`: `$PWD` with every byte written as `%XX`.
- The client turns this into `Proxy-Authorization: Basic base64(<command>:<decoded dir>)`.
- The proxy accepts any CONNECT with or without the header, and never answers 407.
- Validation and limits: [data-model.md](../data-model.md). A label that fails any rule is dropped and the connection proceeds.
- The label is not a secret and not authentication. It is never logged on its own and is written only as the `label` field of `exchange_start`, `unsupported` and `tunnel`.
