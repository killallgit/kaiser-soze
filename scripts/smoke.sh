#!/bin/sh
# Checks the installed always-on service end to end with one real claude call.
# Prints PASS or FAIL per check with counts and ids only, never the agent's output.
# Usage: smoke.sh
root=$(cd "$(dirname "$0")/.." && pwd)
port=${SOZE_PORT:-48731}
data=$HOME/.local/share/soze
tap=$data/tap.jsonl
log=$data/logs/soze.log
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
failed=0

pass() { echo "PASS $*"; }
fail() { echo "FAIL $*"; failed=1; }

sessions() { jq -r '.session // empty' "$tap" 2>/dev/null | sort -u; }
records() { n=$(wc -l 2>/dev/null < "$tap"); echo "${n:-0}"; }
failures() { n=$(grep -cE 'connect_failed|tunnel_copy_failed' "$log" 2>/dev/null); echo "${n:-0}"; }

sh "$root/scripts/service.sh" status 2>/dev/null | grep -qx 'state=running' || { fail service_running; exit 1; }
pass service_running

nc -z -w 1 127.0.0.1 "$port" >/dev/null 2>&1 || { fail port_open port="$port"; exit 1; }
pass port_open port="$port"

sessions > "$work/sessions.before"
records_before=$(records)
failures_before=$(failures)

env -u HTTPS_PROXY -u https_proxy -u ANTHROPIC_BASE_URL \
  HTTPS_PROXY="http://127.0.0.1:$port" NODE_EXTRA_CA_CERTS="$HOME/.config/soze/ca.pem" \
  claude -p "Reply with the single word ok." < /dev/null > "$work/agent" 2>&1 &
agent=$!
# macOS has no timeout(1): a background sleeper kills the agent when the limit passes
(sleep "${SMOKE_TIMEOUT:-120}"; kill "$agent") > /dev/null 2>&1 &
watchdog=$!
wait "$agent"
code=$?
kill "$watchdog" 2>/dev/null
detail="exit=$code bytes=$(wc -c < "$work/agent" | tr -d ' ')"
if [ "$code" -eq 0 ]; then pass agent_call "$detail"; else fail agent_call "$detail"; fi

sessions > "$work/sessions.after"
comm -13 "$work/sessions.before" "$work/sessions.after" > "$work/sessions.new"
new_sessions=$(wc -l < "$work/sessions.new" | tr -d ' ')
new_records=$(($(records) - records_before))
detail="new_sessions=$new_sessions new_records=$new_records session=$(head -n 1 "$work/sessions.new")"
if [ "$new_sessions" -ge 1 ] && [ "$new_records" -ge 1 ]; then pass tap_captured "$detail"; else fail tap_captured "$detail"; fi

new_failures=$(($(failures) - failures_before))
if [ "$new_failures" -eq 0 ]; then pass no_new_failures new_failures=0; else fail no_new_failures new_failures="$new_failures"; fi

exit "$failed"
