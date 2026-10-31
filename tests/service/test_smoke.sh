. "$(dirname "$0")/lib.sh"

MARKER=synthetic-agent-output

fake_nc() {
  cat > "$HOME/bin/nc" <<'FAKE'
#!/bin/sh
echo "$*" >> "$HOME/nc.calls"
[ ! -e "$HOME/nc.closed" ]
FAKE
  chmod +x "$HOME/bin/nc"
}

# Records what it was run with, prints a marker, then a new tap session, a failure line or a
# non-zero exit when $HOME/claude.{nosession,logfail,fail} ask for it, or hangs on claude.hang.
fake_claude() {
  cat > "$HOME/bin/claude" <<'FAKE'
#!/bin/sh
echo call >> "$HOME/claude.calls"
{
  printf 'proxy=%s\n' "$HTTPS_PROXY"
  printf 'lower=%s\n' "$https_proxy"
  printf 'node_ca=%s\n' "$NODE_EXTRA_CA_CERTS"
  printf 'base=%s\n' "$ANTHROPIC_BASE_URL"
  for a in "$@"; do printf 'arg=%s\n' "$a"; done
} > "$HOME/seen.claude"
cat > "$HOME/claude.stdin"
echo synthetic-agent-output
[ ! -e "$HOME/claude.hang" ] || exec sleep 30
[ -e "$HOME/claude.nosession" ] || echo '{"session":"synthetic-new"}' >> "$HOME/.local/share/soze/tap.jsonl"
[ ! -e "$HOME/claude.oldsession" ] || echo '{"session":"synthetic-old"}' >> "$HOME/.local/share/soze/tap.jsonl"
[ ! -e "$HOME/claude.logfail" ] || echo connect_failed >> "$HOME/.local/share/soze/logs/soze.log"
[ ! -e "$HOME/claude.fail" ] || exit 1
FAKE
  chmod +x "$HOME/bin/claude"
}

# ready: a running service, an open port, a logged-in claude and a tap that already holds one session
ready() {
  mkdir -p "$DATA/logs"
  printf '{"session":"synthetic-old"}\n{"session":null}\n{"other":1}\n' > "$DATA/tap.jsonl"
  : > "$DATA/logs/soze.log"
  loaded
  fake_nc
  fake_claude
  export SMOKE_TIMEOUT=5
}

smoke_code() {
  code=0
  sh "$ROOT/scripts/smoke.sh" > "$HOME/out" 2>&1 < /dev/null || code=$?
  echo "$code"
}

claude_calls() {
  if [ -e "$HOME/claude.calls" ]; then wc -l < "$HOME/claude.calls" | tr -d ' '; else echo 0; fi
}

passes_when_the_service_agent_tap_and_log_are_healthy() {
  ready
  eq "$(smoke_code)" 0 "exit status"
  eq "$(grep -c '^FAIL' "$HOME/out")" 0 "failed checks"
  eq "$(grep -c '^PASS' "$HOME/out")" 5 "passed checks"
  eq "$(claude_calls)" 1 "agent calls"
}

fails_without_calling_the_agent_when_the_service_is_not_running() {
  ready
  "$RM" "$HOME/launchctl.loaded"
  [ "$(smoke_code)" -ne 0 ]
  eq "$(claude_calls)" 0 "agent calls"
}

fails_without_calling_the_agent_when_the_port_is_closed() {
  ready
  touch "$HOME/nc.closed"
  [ "$(smoke_code)" -ne 0 ]
  eq "$(claude_calls)" 0 "agent calls"
}

fails_when_the_agent_exits_non_zero() {
  ready
  touch "$HOME/claude.fail"
  [ "$(smoke_code)" -ne 0 ]
}

fails_when_the_agent_outlasts_the_watchdog() {
  ready
  touch "$HOME/claude.hang"
  SMOKE_TIMEOUT=1
  [ "$(smoke_code)" -ne 0 ]
}

fails_when_the_tap_gains_no_new_session() {
  ready
  touch "$HOME/claude.nosession"
  [ "$(smoke_code)" -ne 0 ]
}

fails_when_the_tap_only_repeats_an_old_session() {
  ready
  touch "$HOME/claude.nosession" "$HOME/claude.oldsession"
  [ "$(smoke_code)" -ne 0 ]
}

fails_when_the_log_gains_a_connect_failure() {
  ready
  touch "$HOME/claude.logfail"
  [ "$(smoke_code)" -ne 0 ]
}

never_prints_the_agent_output() {
  ready
  smoke_code > /dev/null
  eq "$(grep -c "$MARKER" "$HOME/out")" 0 "agent output lines"
}

calls_the_agent_through_the_proxy_without_inherited_settings() {
  ready
  export SOZE_PORT=48999 HTTPS_PROXY=http://outer.invalid:1 https_proxy=http://outer.invalid:2 ANTHROPIC_BASE_URL=http://outer.invalid:3
  smoke_code > /dev/null
  eq "$(seen claude proxy)" "http://127.0.0.1:48999" "proxy"
  eq "$(seen claude lower)" "" "lower-case proxy"
  eq "$(seen claude base)" "" "base url"
  eq "$(seen claude node_ca)" "$HOME/.config/soze/ca.pem" "ca"
  eq "$(seen claude arg)" "$(printf '%s\n%s' -p 'Reply with the single word ok.')" "arguments"
  [ ! -s "$HOME/claude.stdin" ]
  grep -q 48999 "$HOME/nc.calls"
}

case_run passes_when_the_service_agent_tap_and_log_are_healthy passes_when_the_service_agent_tap_and_log_are_healthy
case_run fails_without_calling_the_agent_when_the_service_is_not_running fails_without_calling_the_agent_when_the_service_is_not_running
case_run fails_without_calling_the_agent_when_the_port_is_closed fails_without_calling_the_agent_when_the_port_is_closed
case_run fails_when_the_agent_exits_non_zero fails_when_the_agent_exits_non_zero
case_run fails_when_the_agent_outlasts_the_watchdog fails_when_the_agent_outlasts_the_watchdog
case_run fails_when_the_tap_gains_no_new_session fails_when_the_tap_gains_no_new_session
case_run fails_when_the_tap_only_repeats_an_old_session fails_when_the_tap_only_repeats_an_old_session
case_run fails_when_the_log_gains_a_connect_failure fails_when_the_log_gains_a_connect_failure
case_run never_prints_the_agent_output never_prints_the_agent_output
case_run calls_the_agent_through_the_proxy_without_inherited_settings calls_the_agent_through_the_proxy_without_inherited_settings
finish
