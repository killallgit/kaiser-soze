#!/bin/sh
# Runs every wrapper test under each shell that is installed.
cd "$(dirname "$0")" || exit 1
failed=""
for shell in sh bash zsh; do
  command -v "$shell" >/dev/null 2>&1 || continue
  for test in test_*.sh; do
    [ -e "$test" ] || continue
    "$shell" "$test" || failed="$failed $shell:$test"
  done
done
if [ -n "$failed" ]; then
  echo "failed wrapper tests:$failed" >&2
  exit 1
fi
