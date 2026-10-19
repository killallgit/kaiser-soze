#!/bin/sh
cd "$(dirname "$0")/.." || exit 1

timeout_seconds=${DEV_TIMEOUT:-300}
case $timeout_seconds in
  '' | *[!0-9]*) exit 2 ;;
esac
if [ "$timeout_seconds" -lt 1 ] || [ "$timeout_seconds" -gt 600 ]; then
  exit 2
fi

trap : INT
# LIVE=0 turns the live lines off
live=--live
[ "${LIVE:-1}" = 0 ] && live=
mkdir -p target/dev
cargo build || exit 1
# --foreground keeps timeout in this process group so Ctrl-C reaches the proxy
timeout --foreground "$timeout_seconds" "${CARGO_TARGET_DIR:-target}/debug/soze" --tap-file target/dev/tap.jsonl --shape-file target/dev/shapes.jsonl --ca-dir target/dev/ca --port "${SOZE_PORT:-48731}" $live
status=$?
# 124: timeout stopped the proxy; 130: interrupted
if [ "$status" -eq 124 ] || [ "$status" -eq 130 ]; then
  exit 0
fi
exit "$status"
