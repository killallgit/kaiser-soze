#!/usr/bin/env bash
# Blocks `gh pr create` until `task check` passes in the directory the command runs from.
set -u

dir=$(jq -r '.cwd // empty')
[ -n "$dir" ] && cd "$dir" || exit 0

if out=$(timeout 600 task check 2>&1); then
  exit 0
fi

echo "task check failed, so the PR was not opened. Fix it and retry. Last output:" >&2
printf '%s\n' "$out" | tail -40 >&2
exit 2
