# Helpers for the wrapper tests. Source it from a test_*.sh file run with sh, bash or zsh.
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
ORIG_PATH=$PATH
RM=$(command -v rm)
failures=0

new_home() {
  HOME=$(mktemp -d)
  export HOME
  mkdir -p "$HOME/bin" "$HOME/.config/soze"
  PATH="$HOME/bin:$ORIG_PATH"
  unset HTTPS_PROXY https_proxy SOZE_OFF SOZE_PORT SOZE_CA_DIR NODE_EXTRA_CA_CERTS CODEX_CA_CERTIFICATE
}

# A stub agent: records its environment, arguments and stdin under $HOME, exits with status $2.
stub() {
  cat > "$HOME/bin/$1" <<STUB
#!/bin/sh
{
  printf 'proxy=%s\n' "\$HTTPS_PROXY"
  printf 'lower=%s\n' "\$https_proxy"
  printf 'node_ca=%s\n' "\$NODE_EXTRA_CA_CERTS"
  printf 'codex_ca=%s\n' "\$CODEX_CA_CERTIFICATE"
  for a in "\$@"; do printf 'arg=%s\n' "\$a"; done
} > "$HOME/seen.$1"
cat > "$HOME/stdin.$1"
exit $2
STUB
  chmod +x "$HOME/bin/$1"
}

# expected_label <command>: "<command>:<every byte of $PWD as %xx>@", built a different way than the wrapper
expected_label() {
  rest=$PWD
  encoded=
  while [ -n "$rest" ]; do
    char=${rest%"${rest#?}"}
    rest=${rest#?}
    encoded="$encoded$(printf '%%%02x' "'$char")"
  done
  printf '%s:%s@' "$1" "$encoded"
}

# seen <agent> <field>: a recorded value
seen() {
  sed -n "s/^$2=//p" "$HOME/seen.$1"
}

# eq <actual> <expected> <label>: names only the label on a mismatch, never the values
eq() {
  [ "$1" = "$2" ] || {
    echo "  mismatch: $3" >&2
    return 1
  }
}

# case_run <name> <function>: runs the function in a subshell with a fresh throwaway HOME
case_run() {
  (
    set -e
    new_home
    trap '"$RM" -rf "$HOME"' EXIT
    "$2"
  )
  code=$?
  [ "$code" -eq 0 ] || {
    echo "FAIL $1" >&2
    failures=$((failures + 1))
  }
}

finish() {
  [ "$failures" -eq 0 ] || exit 1
}
