. "$(dirname "$0")/lib.sh"

DEV="$ROOT/scripts/dev.sh"

# A stub cargo: logs every subcommand, saves the arguments of `run`, and exits $1.
cargo_stub() {
  cat > "$HOME/bin/cargo" <<STUB
#!/bin/sh
echo "\$1" >> "$HOME/cargo.calls"
if [ "\$1" = run ]; then printf '%s\n' "\$@" > "$HOME/cargo.run"; fi
exit $1
STUB
  chmod +x "$HOME/bin/cargo"
}

timeout_stub() {
  cat > "$HOME/bin/timeout" <<'STUB'
#!/bin/sh
touch "$HOME/timeout.called"
exit 97
STUB
  chmod +x "$HOME/bin/timeout"
}

# arg_after <flag>: the argument that follows the flag in the saved `cargo run` command
arg_after() {
  sed -n "/^$1\$/{n;p;}" "$HOME/cargo.run"
}

starts_the_proxy_through_cargo_run_without_a_timer() {
  cargo_stub 0
  timeout_stub
  sh "$DEV"
  eq "$(cat "$HOME/cargo.calls")" run "one cargo run builds and launches"
  eq "$(arg_after --tap-file)" target/dev/tap.jsonl "tap file"
  eq "$(arg_after --log-file)" target/dev/debug.jsonl "debug log file"
  eq "$(arg_after --port)" 48732 "default port"
  [ ! -e "$HOME/timeout.called" ]
}

the_port_can_be_named() {
  cargo_stub 0
  SOZE_PORT=48999 sh "$DEV"
  eq "$(arg_after --port)" 48999 "named port"
}

a_cargo_failure_propagates_its_status() {
  cargo_stub 23
  code=0
  sh "$DEV" || code=$?
  eq "$code" 23 "cargo failure status"
  eq "$(cat "$HOME/cargo.calls")" run "cargo handles build and startup"
}

an_inherited_dev_timeout_does_not_schedule_a_shutdown() {
  cargo_stub 0
  timeout_stub
  DEV_TIMEOUT=1 sh "$DEV"
  eq "$(cat "$HOME/cargo.calls")" run "old timeout setting is ignored"
  [ ! -e "$HOME/timeout.called" ]
}

stream_and_logging_arguments_reach_the_binary() {
  cargo_stub 0
  sh "$DEV" --stream --log-level debug --log-format json
  eq "$(arg_after --log-level)" debug "log level"
  eq "$(arg_after --log-format)" json "log format"
  eq "$(sed -n '/^--stream$/p' "$HOME/cargo.run")" --stream "stream flag"
}

case_run starts_the_proxy_through_cargo_run_without_a_timer starts_the_proxy_through_cargo_run_without_a_timer
case_run the_port_can_be_named the_port_can_be_named
case_run a_cargo_failure_propagates_its_status a_cargo_failure_propagates_its_status
case_run an_inherited_dev_timeout_does_not_schedule_a_shutdown an_inherited_dev_timeout_does_not_schedule_a_shutdown
case_run stream_and_logging_arguments_reach_the_binary stream_and_logging_arguments_reach_the_binary
finish
