# Sends claude, codex and agent through the soze proxy when it is running. In every other case
# the real command runs untouched. Sourced by zsh and bash; keep it POSIX sh.
#   SOZE_PORT    proxy port, default 48731
#   SOZE_CA_DIR  directory holding ca.pem, default ~/.config/soze
#   SOZE_OFF     any value skips the wrappers for that command

_soze_probe() {
  command -v nc >/dev/null 2>&1 && nc -z -w 1 127.0.0.1 "${SOZE_PORT:-48731}" >/dev/null 2>&1
}

_soze_ready() {
  [ -z "${SOZE_OFF:-}" ] && [ -z "${HTTPS_PROXY:-}${https_proxy:-}" ] &&
    _soze_probe && [ -r "${SOZE_CA_DIR:-$HOME/.config/soze}/ca.pem" ]
}

# Prints "<command>:<every byte of $PWD as %xx>@" for the proxy address, which tells soze which agent
# and directory started the connection. Prints nothing if it cannot, so the plain address is used.
_soze_label() (
  hex=$(printf '%s' "$PWD" | od -An -tx1 | tr -d ' \n' | sed 's/../%&/g') || exit 0
  [ -n "$hex" ] || exit 0
  printf '%s:%s@' "$1" "$hex"
)

claude() {
  if _soze_ready 2>/dev/null; then
    env HTTPS_PROXY="http://$(_soze_label claude 2>/dev/null)127.0.0.1:${SOZE_PORT:-48731}" \
      NODE_EXTRA_CA_CERTS="${SOZE_CA_DIR:-$HOME/.config/soze}/ca.pem" claude "$@"
  else
    command claude "$@"
  fi
}

codex() {
  if _soze_ready 2>/dev/null; then
    env HTTPS_PROXY="http://$(_soze_label codex 2>/dev/null)127.0.0.1:${SOZE_PORT:-48731}" \
      CODEX_CA_CERTIFICATE="${SOZE_CA_DIR:-$HOME/.config/soze}/ca.pem" codex "$@"
  else
    command codex "$@"
  fi
}

# Cursor's host is tunnelled, not decrypted, so it needs the proxy but no CA.
agent() {
  if _soze_ready 2>/dev/null; then
    env HTTPS_PROXY="http://$(_soze_label agent 2>/dev/null)127.0.0.1:${SOZE_PORT:-48731}" agent "$@"
  else
    command agent "$@"
  fi
}
