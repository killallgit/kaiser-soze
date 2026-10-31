. "$(dirname "$0")/lib.sh"

SHA=0123456789abcdef0123456789abcdef01234567

# expected_record <sha> <branch> <dirty>: install.json without its installed_at line
expected_record() {
  printf '{\n  "source": "local",\n  "sha": "%s",\n  "branch": "%s",\n  "dirty": %s,\n}\n' "$1" "$2" "$3"
}

record_without_time() {
  sed '/"installed_at"/d' "$DATA/bin/install.json"
}

has_utc_install_time() {
  sed -n 6p "$DATA/bin/install.json" |
    grep -qE '^  "installed_at": "[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z"$'
}

install_fake() {
  fake_soze "$HOME/soze" "${1:-one}"
  service install "$HOME/soze"
}

inode() {
  ls -i "$1" | awk '{ print $1 }'
}

writes_the_plist_and_the_binary_then_bootstraps() {
  install_fake
  expected_plist > "$HOME/expected.plist"
  cmp "$HOME/expected.plist" "$PLIST"
  cmp "$HOME/soze" "$DATA/bin/soze"
  [ -x "$DATA/bin/soze" ]
  eq "$(calls)" "$(printf 'print %s\nbootstrap %s %s' "$DOMAIN/$LABEL" "$DOMAIN" "$PLIST")" "launchctl calls"
}

records_the_build_in_install_json() {
  install_fake
  eq "$(record_without_time)" "$(expected_record "$SHA" main false)" "install record"
  has_utc_install_time
}

a_dirty_tree_is_recorded() {
  touch "$HOME/git.dirty"
  install_fake
  eq "$(record_without_time)" "$(expected_record "$SHA" main true)" "install record"
}

a_quote_in_the_branch_is_escaped() {
  printf 'a"b\n' > "$HOME/git.branch"
  install_fake
  eq "$(sed -n 4p "$DATA/bin/install.json")" '  "branch": "a\"b",' "branch line"
}

the_binary_is_installed_with_mode_755() {
  fake_soze "$HOME/soze" one
  chmod 700 "$HOME/soze"
  service install "$HOME/soze"
  [ -n "$(find "$DATA/bin/soze" -perm 755)" ]
}

a_git_failure_records_unknown() {
  printf '#!/bin/sh\nexit 1\n' > "$HOME/bin/git"
  install_fake
  eq "$(record_without_time)" "$(expected_record unknown unknown false)" "install record"
  has_utc_install_time
}

creates_the_config_when_absent() {
  install_fake
  eq "$(cat "$HOME/.config/soze/config.toml")" "$(printf 'log_format = "human"\nlive = true')" config
}

keeps_an_existing_config() {
  mkdir -p "$HOME/.config/soze"
  printf 'live = false\n' > "$HOME/.config/soze/config.toml"
  cp "$HOME/.config/soze/config.toml" "$HOME/config.before"
  install_fake
  cmp "$HOME/config.before" "$HOME/.config/soze/config.toml"
}

rotates_the_log_by_one_generation() {
  install_fake
  printf 'A\n' > "$DATA/logs/soze.log"
  install_fake
  eq "$(cat "$DATA/logs/soze.log.1")" A "first rotation"
  [ ! -e "$DATA/logs/soze.log" ]
  printf 'B\n' > "$DATA/logs/soze.log"
  install_fake
  eq "$(cat "$DATA/logs/soze.log.1")" B "second rotation"
  [ ! -e "$DATA/logs/soze.log.2" ]
}

a_second_install_changes_nothing_and_kickstarts() {
  install_fake
  cp "$PLIST" "$HOME/plist.one"
  cp "$DATA/bin/soze" "$HOME/soze.one"
  cp "$HOME/.config/soze/config.toml" "$HOME/config.one"
  "$RM" "$HOME/launchctl.calls"
  install_fake
  cmp "$HOME/plist.one" "$PLIST"
  cmp "$HOME/soze.one" "$DATA/bin/soze"
  cmp "$HOME/config.one" "$HOME/.config/soze/config.toml"
  eq "$(calls)" "$(printf 'print %s\nkickstart -k %s' "$DOMAIN/$LABEL" "$DOMAIN/$LABEL")" "launchctl calls"
}

an_update_installs_the_new_binary_as_a_new_file() {
  install_fake one
  before=$(inode "$DATA/bin/soze")
  install_fake two
  cmp "$HOME/soze" "$DATA/bin/soze"
  [ "$(inode "$DATA/bin/soze")" != "$before" ]
}

a_changed_plist_while_loaded_boots_out_then_bootstraps() {
  loaded
  install_fake
  expected_plist > "$HOME/expected.plist"
  cmp "$HOME/expected.plist" "$PLIST"
  eq "$(calls)" "$(printf 'print %s\nbootout %s\nbootstrap %s %s' "$DOMAIN/$LABEL" "$DOMAIN/$LABEL" "$DOMAIN" "$PLIST")" "launchctl calls"
}

a_failing_bootstrap_fails_the_install() {
  echo bootstrap > "$HOME/launchctl.fail"
  code=0
  install_fake || code=$?
  [ "$code" -ne 0 ]
}

# rejected <args...>: exits 2, calls no launchctl and writes nothing
rejected() {
  code=0
  service "$@" 2>/dev/null || code=$?
  eq "$code" 2 "exit status"
  [ ! -e "$HOME/launchctl.calls" ]
  [ ! -e "$HOME/.local" ]
  [ ! -e "$HOME/.config" ]
  [ ! -e "$HOME/Library" ]
}

bad_arguments_exit_2_and_change_nothing() {
  fake_soze "$HOME/soze" one
  rejected
  rejected install
  rejected install "$HOME/soze" extra
  rejected frobnicate
}

a_bad_binary_exits_2_and_changes_nothing() {
  printf 'not executable\n' > "$HOME/plain"
  rejected install "$HOME/missing"
  rejected install "$HOME/bin"
  rejected install "$HOME/plain"
}

nothing_is_written_under_agent_folders() {
  install_fake
  [ ! -e "$HOME/.claude" ]
  [ ! -e "$HOME/.codex" ]
  [ ! -e "$HOME/.cursor" ]
}

nothing_is_written_outside_the_home() {
  install_fake one
  install_fake two
  eq "$(find "$ROOT" -newer "$HOME/.marker" -not -path '*/.git/*' -not -path '*/target/*')" "" "files written in the repo"
}

case_run writes_the_plist_and_the_binary_then_bootstraps writes_the_plist_and_the_binary_then_bootstraps
case_run records_the_build_in_install_json records_the_build_in_install_json
case_run a_dirty_tree_is_recorded a_dirty_tree_is_recorded
case_run a_quote_in_the_branch_is_escaped a_quote_in_the_branch_is_escaped
case_run the_binary_is_installed_with_mode_755 the_binary_is_installed_with_mode_755
case_run a_git_failure_records_unknown a_git_failure_records_unknown
case_run creates_the_config_when_absent creates_the_config_when_absent
case_run keeps_an_existing_config keeps_an_existing_config
case_run rotates_the_log_by_one_generation rotates_the_log_by_one_generation
case_run a_second_install_changes_nothing_and_kickstarts a_second_install_changes_nothing_and_kickstarts
case_run an_update_installs_the_new_binary_as_a_new_file an_update_installs_the_new_binary_as_a_new_file
case_run a_changed_plist_while_loaded_boots_out_then_bootstraps a_changed_plist_while_loaded_boots_out_then_bootstraps
case_run a_failing_bootstrap_fails_the_install a_failing_bootstrap_fails_the_install
case_run bad_arguments_exit_2_and_change_nothing bad_arguments_exit_2_and_change_nothing
case_run a_bad_binary_exits_2_and_changes_nothing a_bad_binary_exits_2_and_changes_nothing
case_run nothing_is_written_under_agent_folders nothing_is_written_under_agent_folders
case_run nothing_is_written_outside_the_home nothing_is_written_outside_the_home
finish
