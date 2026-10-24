. "$(dirname "$0")/lib.sh"

DEV="$ROOT/scripts/dev.sh"

# A stub cargo: logs every subcommand, saves the arguments of `run`, and exits $1 for `build`.
cargo_stub() {
  cat > "$HOME/bin/cargo" <<STUB
#!/bin/sh
echo "\$1" >> "$HOME/cargo.calls"
if [ "\$1" = run ]; then printf '%s\n' "\$@" > "$HOME/cargo.run"; fi
if [ "\$1" = build ]; then exit $1; fi
exit 0
STUB
  chmod +x "$HOME/bin/cargo"
}

# arg_after <flag>: the argument that follows the flag in the saved `cargo run` command
arg_after() {
  sed -n "/^$1\$/{n;p;}" "$HOME/cargo.run"
}

starts_the_proxy_through_cargo_run() {
  cargo_stub 0
  DEV_TIMEOUT=2 sh "$DEV"
  eq "$(sed -n 1p "$HOME/cargo.calls")" build "build first"
  eq "$(sed -n 2p "$HOME/cargo.calls")" run "run second"
  eq "$(arg_after --tap-file)" target/dev/tap.jsonl "tap file"
  eq "$(arg_after --port)" 48731 "default port"
}

the_port_can_be_named() {
  cargo_stub 0
  SOZE_PORT=48999 DEV_TIMEOUT=2 sh "$DEV"
  eq "$(arg_after --port)" 48999 "named port"
}

a_failed_build_runs_nothing() {
  cargo_stub 1
  code=0
  DEV_TIMEOUT=2 sh "$DEV" || code=$?
  eq "$code" 1 "build status"
  [ ! -e "$HOME/cargo.run" ]
}

a_timeout_out_of_range_builds_nothing() {
  cargo_stub 0
  code=0
  DEV_TIMEOUT=601 sh "$DEV" || code=$?
  eq "$code" 2 "timeout status"
  [ ! -e "$HOME/cargo.calls" ]
}

case_run starts_the_proxy_through_cargo_run starts_the_proxy_through_cargo_run
case_run the_port_can_be_named the_port_can_be_named
case_run a_failed_build_runs_nothing a_failed_build_runs_nothing
case_run a_timeout_out_of_range_builds_nothing a_timeout_out_of_range_builds_nothing
finish
