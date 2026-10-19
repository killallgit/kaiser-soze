. "$(dirname "$0")/lib.sh"

ready_home() {
  printf 'ca' > "$HOME/.config/soze/ca.pem"
  stub claude 0
  stub codex 0
  stub agent 0
  . "$ROOT/scripts/soze-wrappers.sh"
  _soze_probe() { return 0; }
}

claude_gets_the_proxy_and_the_node_ca() {
  ready_home
  claude </dev/null
  eq "$(seen claude proxy)" "http://127.0.0.1:48731" proxy
  eq "$(seen claude node_ca)" "$HOME/.config/soze/ca.pem" node_ca
  eq "$(seen claude codex_ca)" "" codex_ca
}

codex_gets_the_proxy_and_the_codex_ca() {
  ready_home
  codex </dev/null
  eq "$(seen codex proxy)" "http://127.0.0.1:48731" proxy
  eq "$(seen codex codex_ca)" "$HOME/.config/soze/ca.pem" codex_ca
  eq "$(seen codex node_ca)" "" node_ca
}

agent_gets_the_proxy_only() {
  ready_home
  agent </dev/null
  eq "$(seen agent proxy)" "http://127.0.0.1:48731" proxy
  eq "$(seen agent node_ca)" "" node_ca
  eq "$(seen agent codex_ca)" "" codex_ca
}

port_and_ca_dir_can_be_overridden() {
  ready_home
  mkdir "$HOME/other"
  printf 'ca' > "$HOME/other/ca.pem"
  SOZE_PORT=50123
  SOZE_CA_DIR="$HOME/other"
  claude </dev/null
  eq "$(seen claude proxy)" "http://127.0.0.1:50123" proxy
  eq "$(seen claude node_ca)" "$HOME/other/ca.pem" node_ca
}

arguments_status_and_stdin_pass_through() {
  ready_home
  stub claude 7
  code=0
  printf 'typed' | claude "a b" -p c || code=$?
  eq "$code" 7 status
  eq "$(sed -n 's/^arg=//p' "$HOME/seen.claude" | tr '\n' '|')" "a b|-p|c|" arguments
  eq "$(cat "$HOME/stdin.claude")" typed stdin
}

the_variables_are_not_left_in_the_shell() {
  ready_home
  claude </dev/null
  eq "${HTTPS_PROXY:-}" "" HTTPS_PROXY
  eq "${NODE_EXTRA_CA_CERTS:-}" "" NODE_EXTRA_CA_CERTS
}

case_run claude_gets_the_proxy_and_the_node_ca claude_gets_the_proxy_and_the_node_ca
case_run codex_gets_the_proxy_and_the_codex_ca codex_gets_the_proxy_and_the_codex_ca
case_run agent_gets_the_proxy_only agent_gets_the_proxy_only
case_run port_and_ca_dir_can_be_overridden port_and_ca_dir_can_be_overridden
case_run arguments_status_and_stdin_pass_through arguments_status_and_stdin_pass_through
case_run the_variables_are_not_left_in_the_shell the_variables_are_not_left_in_the_shell
finish
