# Data model

**Settings** (environment, read at each call):
- `SOZE_PORT`: proxy port, default 29817
- `SOZE_CA_DIR`: directory holding `ca.pem`, default `$HOME/.config/soze`
- `SOZE_OFF`: any non-empty value skips the wrapper for that command

**Wrapper table**:

| Command | Variables set for that command |
|---|---|
| `claude` | `HTTPS_PROXY=http://127.0.0.1:$SOZE_PORT`, `NODE_EXTRA_CA_CERTS=$SOZE_CA_DIR/ca.pem` |
| `codex` | `HTTPS_PROXY=http://127.0.0.1:$SOZE_PORT`, `CODEX_CA_CERTIFICATE=$SOZE_CA_DIR/ca.pem` |
| `agent` | `HTTPS_PROXY=http://127.0.0.1:$SOZE_PORT` |

**Install state**: the file `~/.config/soze/wrappers.sh` and the marked block in the start-up file. Installed means both exist; absent means neither. A start-up file with a block but no file, or the reverse, is repaired by install and cleared by uninstall.

**Ready**: true only when all hold: `SOZE_OFF` empty, `HTTPS_PROXY` and `https_proxy` unset, `nc` found, the port accepts a connection, the CA file is readable.
