. "$(dirname "$0")/lib.sh"

# Every case runs `claude one "t w"` and expects the stub to run with its own arguments, exit
# status 3 and no output of the wrapper's own.
untouched_home() {
  printf 'ca' > "$HOME/.config/soze/ca.pem"
  stub claude 3
  . "$ROOT/scripts/soze-wrappers.sh"
}

run_claude() {
  code=0
  claude one "t w" </dev/null >"$HOME/out" 2>&1 || code=$?
}

assert_untouched() {
  eq "$code" 3 status
  eq "$(seen claude proxy)" "${1:-}" proxy
  eq "$(seen claude node_ca)" "" node_ca
  eq "$(sed -n 's/^arg=//p' "$HOME/seen.claude" | tr '\n' '|')" "one|t w|" arguments
  eq "$(wc -c < "$HOME/out" | tr -d ' ')" 0 output
}

nothing_listening() {
  untouched_home
  SOZE_PORT=1
  run_claude
  assert_untouched
}

probe_missing() {
  untouched_home
  unset -f _soze_probe
  run_claude
  assert_untouched
}

nc_not_installed() {
  untouched_home
  ln -s "$(command -v env)" "$HOME/bin/env"
  ln -s "$(command -v cat)" "$HOME/bin/cat"
  saved=$PATH
  PATH="$HOME/bin"
  run_claude
  PATH=$saved
  assert_untouched
}

ca_missing() {
  untouched_home
  _soze_probe() { return 0; }
  "$RM" "$HOME/.config/soze/ca.pem"
  run_claude
  assert_untouched
}

ca_unreadable() {
  [ "$(id -u)" -ne 0 ] || return 0
  untouched_home
  _soze_probe() { return 0; }
  chmod 000 "$HOME/.config/soze/ca.pem"
  run_claude
  assert_untouched
}

https_proxy_already_set() {
  untouched_home
  _soze_probe() { return 0; }
  export HTTPS_PROXY=http://other.invalid:1
  run_claude
  assert_untouched http://other.invalid:1
}

lowercase_https_proxy_already_set() {
  untouched_home
  _soze_probe() { return 0; }
  export https_proxy=http://other.invalid:1
  run_claude
  assert_untouched
  eq "$(seen claude lower)" http://other.invalid:1 lower
}

switched_off() {
  untouched_home
  _soze_probe() { return 0; }
  SOZE_OFF=1
  run_claude
  assert_untouched
}

helper_undefined() {
  untouched_home
  unset -f _soze_ready
  run_claude
  assert_untouched
}

case_run nothing_listening nothing_listening
case_run probe_missing probe_missing
case_run nc_not_installed nc_not_installed
case_run ca_missing ca_missing
case_run ca_unreadable ca_unreadable
case_run https_proxy_already_set https_proxy_already_set
case_run lowercase_https_proxy_already_set lowercase_https_proxy_already_set
case_run switched_off switched_off
case_run helper_undefined helper_undefined
finish
