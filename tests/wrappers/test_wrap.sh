. "$(dirname "$0")/lib.sh"

WRAP="$ROOT/scripts/soze-wrap"

# nc stands in for the proxy probe: exit status $1
probe() {
  printf '#!/bin/sh\nexit %s\n' "$1" > "$HOME/bin/nc"
  chmod +x "$HOME/bin/nc"
}

ready_home() {
  printf 'ca' > "$HOME/.config/soze/ca.pem"
  cp "$ROOT/scripts/soze-wrappers.sh" "$HOME/.config/soze/wrappers.sh"
  stub claude 0
  stub codex 0
  stub agent 0
  probe 0
}

runs_each_agent_through_the_proxy() {
  ready_home
  for name in claude codex agent; do
    sh "$WRAP" "$name" </dev/null
    eq "$(seen "$name" proxy)" "http://$(expected_label "$name")127.0.0.1:48731" "proxy $name"
  done
}

arguments_status_and_stdin_pass_through() {
  ready_home
  stub claude 7
  code=0
  printf 'typed' | sh "$WRAP" claude "a b" -p c || code=$?
  eq "$code" 7 status
  eq "$(sed -n 's/^arg=//p' "$HOME/seen.claude" | tr '\n' '|')" "a b|-p|c|" arguments
  eq "$(cat "$HOME/stdin.claude")" typed stdin
}

with_nothing_listening_the_real_command_runs_untouched() {
  ready_home
  probe 1
  sh "$WRAP" claude </dev/null
  eq "$(seen claude proxy)" "" proxy
}

without_the_wrappers_file_the_real_command_runs_untouched() {
  ready_home
  "$RM" "$HOME/.config/soze/wrappers.sh"
  sh "$WRAP" claude </dev/null
  eq "$(seen claude proxy)" "" proxy
}

the_wrappers_file_can_be_named() {
  ready_home
  "$RM" "$HOME/.config/soze/wrappers.sh"
  SOZE_WRAPPERS="$ROOT/scripts/soze-wrappers.sh" sh "$WRAP" codex </dev/null
  eq "$(seen codex proxy)" "http://$(expected_label codex)127.0.0.1:48731" proxy
}

another_name_fails_and_runs_nothing() {
  ready_home
  stub ls 0
  code=0
  sh "$WRAP" ls </dev/null 2>/dev/null || code=$?
  eq "$code" 2 status
  [ ! -e "$HOME/seen.ls" ]
  code=0
  sh "$WRAP" </dev/null 2>/dev/null || code=$?
  eq "$code" 2 status
}

case_run runs_each_agent_through_the_proxy runs_each_agent_through_the_proxy
case_run arguments_status_and_stdin_pass_through arguments_status_and_stdin_pass_through
case_run with_nothing_listening_the_real_command_runs_untouched with_nothing_listening_the_real_command_runs_untouched
case_run without_the_wrappers_file_the_real_command_runs_untouched without_the_wrappers_file_the_real_command_runs_untouched
case_run the_wrappers_file_can_be_named the_wrappers_file_can_be_named
case_run another_name_fails_and_runs_nothing another_name_fails_and_runs_nothing
finish
