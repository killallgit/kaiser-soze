. "$(dirname "$0")/lib.sh"

HOOK="$ROOT/.claude/hooks/pre-pr-check.sh"

# hook_runs <command>: feeds the hook the JSON a Bash tool call carries and returns its exit status
hook_runs() {
  jq -n --arg dir "$HOME" --arg command "$1" '{cwd: $dir, tool_input: {command: $command}}' | "$HOOK" 2>/dev/null
}

# task_stub <status>: a task that records that it ran and exits with the status
task_stub() {
  printf '#!/bin/sh\ntouch "%s/task.ran"\nexit %s\n' "$HOME" "$1" > "$HOME/bin/task"
  chmod +x "$HOME/bin/task"
}

a_command_that_is_not_a_pr_create_never_runs_the_check() {
  task_stub 1
  hook_runs 'cargo fmt && timeout 600 task check > /tmp/log 2>&1; echo exit=$?'
  eq "$([ -e "$HOME/task.ran" ] && echo ran)" "" check_ran
}

a_pr_create_with_a_failing_check_is_blocked() {
  task_stub 1
  code=0
  hook_runs 'gh pr create --title t --body b' || code=$?
  eq "$code" 2 exit_code
}

a_chained_pr_create_runs_the_check_and_passes_when_it_does() {
  task_stub 0
  hook_runs 'git push -u origin branch && gh pr create --title t --body b'
  eq "$([ -e "$HOME/task.ran" ] && echo ran)" ran check_ran
}

case_run non_pr_command_skips_check a_command_that_is_not_a_pr_create_never_runs_the_check
case_run failing_check_blocks_pr a_pr_create_with_a_failing_check_is_blocked
case_run chained_pr_create_runs_check a_chained_pr_create_runs_the_check_and_passes_when_it_does
finish
