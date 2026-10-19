#!/bin/sh
cd "$(dirname "$0")/.." || exit 1

failed=""
run() {
  name=$1
  shift
  "$@" || failed="$failed $name"
}

run fmt cargo fmt --check
run clippy cargo clippy --all-targets -- -D warnings
run test cargo test
run wrappers tests/wrappers/run.sh

if [ -n "$failed" ]; then
  echo "failed checks:$failed" >&2
  exit 1
fi
