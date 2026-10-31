. "$(dirname "$0")/lib.sh"

CONTENT=synthetic-capture-content

# installed: soze installed and loaded, with the launchctl calls of the install forgotten
installed() {
  fake_soze "$HOME/soze" one
  service install "$HOME/soze"
  "$RM" "$HOME/launchctl.calls"
}

with_captures() {
  for file in tap.jsonl shapes.jsonl debug.jsonl logs/soze.log; do
    printf '{"text":"%s"}\n' "$CONTENT" > "$DATA/$file"
  done
}

captures_listing() {
  printf '%s\n' "$DATA/debug.jsonl" "$DATA/logs" "$DATA/logs/soze.log" "$DATA/shapes.jsonl" "$DATA/tap.jsonl" | sort
}

exit_code() {
  code=0
  service "$@" > "$HOME/out" 2>/dev/null || code=$?
  echo "$code"
}

restart_kickstarts_the_agent() {
  loaded
  service restart
  eq "$(calls)" "kickstart -k $DOMAIN/$LABEL" "launchctl calls"
}

restart_fails_when_nothing_is_loaded() {
  [ "$(exit_code restart)" -ne 0 ]
  eq "$(calls)" "kickstart -k $DOMAIN/$LABEL" "launchctl calls"
}

reload_sends_hup_to_the_agent() {
  loaded
  service reload
  eq "$(calls)" "kill HUP $DOMAIN/$LABEL" "launchctl calls"
}

reload_fails_when_nothing_is_loaded() {
  [ "$(exit_code reload)" -ne 0 ]
  eq "$(calls)" "kill HUP $DOMAIN/$LABEL" "launchctl calls"
}

status_shows_the_state_and_the_install_record() {
  mkdir -p "$DATA/bin"
  printf '{\n  "source": "local",\n  "sha": "abc123",\n  "branch": "feature",\n  "dirty": true,\n  "installed_at": "2026-01-02T03:04:05Z"\n}\n' > "$DATA/bin/install.json"
  loaded
  service status > "$HOME/out"
  eq "$(cat "$HOME/out")" "$(printf 'state=running\npid=4242\nsource=local\nsha=abc123\nbranch=feature\ndirty=true\ninstalled_at=2026-01-02T03:04:05Z')" status
}

status_without_an_install_shows_not_loaded() {
  service status > "$HOME/out"
  eq "$(cat "$HOME/out")" "$(printf 'state=not loaded\npid=')" status
}

logs_follows_only_the_service_log() {
  fake_tail
  service logs
  eq "$(cat "$HOME/tail.args")" "$(printf '%s\n%s' -F "$DATA/logs/soze.log")" "tail arguments"
}

uninstall_removes_the_service_and_keeps_the_data() {
  installed
  with_captures
  service uninstall > "$HOME/out"
  eq "$(calls)" "bootout $DOMAIN/$LABEL" "launchctl calls"
  [ ! -e "$PLIST" ]
  [ ! -e "$DATA/bin" ]
  eq "$(find "$DATA" -mindepth 1 | sort)" "$(captures_listing)" "kept data"
  [ -e "$HOME/.config/soze/config.toml" ]
  [ ! -s "$HOME/out" ]
}

uninstall_succeeds_when_nothing_is_installed() {
  service uninstall
  eq "$(calls)" "bootout $DOMAIN/$LABEL" "launchctl calls"
}

purge_lists_the_data_paths_then_deletes_the_data() {
  installed
  with_captures
  service uninstall --purge > "$HOME/out"
  eq "$(sort "$HOME/out")" "$(captures_listing)" "listed paths"
  eq "$(grep -c "$CONTENT" "$HOME/out")" 0 content
  [ ! -e "$DATA" ]
  [ ! -e "$PLIST" ]
  [ -e "$HOME/.config/soze/config.toml" ]
}

# refused_without_home <args...>: with HOME empty and with HOME unset, exits 2, calls no launchctl and changes nothing
refused_without_home() {
  installed
  with_captures
  printf '#!/bin/sh\ntouch "%s/launchctl.called"\n' "$HOME" > "$HOME/bin/launchctl"
  find "$HOME" | sort > "$HOME/before"
  code=0
  env HOME= sh "$ROOT/scripts/service.sh" "$@" > /dev/null 2>&1 || code=$?
  eq "$code" 2 "exit status with an empty HOME"
  code=0
  env -u HOME sh "$ROOT/scripts/service.sh" "$@" > /dev/null 2>&1 || code=$?
  eq "$code" 2 "exit status with no HOME"
  eq "$(find "$HOME" | sort)" "$(cat "$HOME/before")" "files"
}

uninstall_without_a_home_changes_nothing() {
  refused_without_home uninstall
}

purge_without_a_home_deletes_nothing() {
  refused_without_home uninstall --purge
}

extra_arguments_exit_2_and_call_nothing() {
  eq "$(exit_code restart now)" 2 restart
  eq "$(exit_code reload now)" 2 reload
  eq "$(exit_code status now)" 2 status
  eq "$(exit_code logs now)" 2 logs
  eq "$(exit_code uninstall --force)" 2 uninstall
  eq "$(exit_code uninstall --purge now)" 2 purge
  [ ! -e "$HOME/launchctl.calls" ]
}

case_run restart_kickstarts_the_agent restart_kickstarts_the_agent
case_run restart_fails_when_nothing_is_loaded restart_fails_when_nothing_is_loaded
case_run reload_sends_hup_to_the_agent reload_sends_hup_to_the_agent
case_run reload_fails_when_nothing_is_loaded reload_fails_when_nothing_is_loaded
case_run status_shows_the_state_and_the_install_record status_shows_the_state_and_the_install_record
case_run status_without_an_install_shows_not_loaded status_without_an_install_shows_not_loaded
case_run logs_follows_only_the_service_log logs_follows_only_the_service_log
case_run uninstall_removes_the_service_and_keeps_the_data uninstall_removes_the_service_and_keeps_the_data
case_run uninstall_succeeds_when_nothing_is_installed uninstall_succeeds_when_nothing_is_installed
case_run purge_lists_the_data_paths_then_deletes_the_data purge_lists_the_data_paths_then_deletes_the_data
case_run uninstall_without_a_home_changes_nothing uninstall_without_a_home_changes_nothing
case_run purge_without_a_home_deletes_nothing purge_without_a_home_deletes_nothing
case_run extra_arguments_exit_2_and_call_nothing extra_arguments_exit_2_and_call_nothing
finish
