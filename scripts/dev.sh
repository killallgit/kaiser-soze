#!/bin/sh
cd "$(dirname "$0")/.." || exit 1

# LIVE=0 turns the live lines off
live=--live
[ "${LIVE:-1}" = 0 ] && live=
mkdir -p target/dev || exit 1
# cargo run replaces itself with the binary it just built, wherever cargo puts it.
exec cargo run --quiet -- --tap-file target/dev/tap.jsonl --shape-file target/dev/shapes.jsonl --log-file target/dev/debug.jsonl --port "${SOZE_PORT:-48731}" $live "$@"
