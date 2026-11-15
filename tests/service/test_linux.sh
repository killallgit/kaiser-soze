. "$(dirname "$0")/lib.sh"
SERVICE_OS=Linux

install_fake() {
  fake_soze "$HOME/soze" "${1:-one}"
  service install "$HOME/soze"
}

systemctl_calls() { cat "$HOME/systemctl.calls"; }

installed() {
  install_fake
  "$RM" "$HOME/systemctl.calls"
}

exit_code() {
  code=0
  service "$@" > "$HOME/out" 2>/dev/null || code=$?
  echo "$code"
}

installs_an_enabled_always_on_user_unit() {
  install_fake

  # The user unit supervises the foreground binary, reloads logging and keeps output private.
  [ -x "$DATA/bin/soze" ]
  cmp "$HOME/soze" "$DATA/bin/soze"
  eq "$(systemctl_calls)" "$(printf '%s\n' 'show --property=UnitPath --value' daemon-reload 'enable soze.service' 'restart soze.service')" "install calls"
  eq "$(cat "$UNIT")" "$(cat <<'UNIT'
[Unit]
Description=Soze local model API proxy

[Service]
Type=simple
ExecStart="%h/.local/share/soze/bin/soze" --port 48731 --tap-file "%h/.local/share/soze/tap.jsonl" --shape-file "%h/.local/share/soze/shapes.jsonl" --log-file "%h/.local/share/soze/debug.jsonl"
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=5
UMask=0077
StandardOutput=append:%h/.local/share/soze/logs/soze.log
StandardError=inherit

[Install]
WantedBy=default.target
UNIT
)" "unit definition"
  [ ! -e "$PLIST" ]
  [ ! -e "$HOME/launchctl.calls" ]
}

shares_config_and_provenance_with_macos() {
  install_fake
  service status > "$HOME/out"

  # Linux uses the same defaults and installation record, with the smoke-compatible state.
  eq "$(cat "$HOME/.config/soze/config.toml")" "$(printf 'log_format = "human"\nlive = true')" config
  grep -qx 'state=running' "$HOME/out"
  grep -qx 'pid=4242' "$HOME/out"
  grep -qx 'sha=0123456789abcdef0123456789abcdef01234567' "$HOME/out"
  grep -qx 'branch=main' "$HOME/out"
  grep -qx 'dirty=false' "$HOME/out"
}

updates_the_binary_preserving_config_and_captures() {
  installed
  printf 'live = false\n' > "$HOME/.config/soze/config.toml"
  cp "$HOME/.config/soze/config.toml" "$HOME/config.before"
  printf '{"synthetic":true}\n' > "$DATA/tap.jsonl"
  cp "$DATA/tap.jsonl" "$HOME/tap.before"
  printf 'synthetic\n' > "$DATA/logs/soze.log"
  cp "$DATA/logs/soze.log" "$HOME/log.before"
  before=$(ls -i "$DATA/bin/soze" | awk '{print $1}')

  install_fake two

  # Updates replace the executable inode and rotate only the operational log.
  cmp "$HOME/soze" "$DATA/bin/soze"
  [ "$(ls -i "$DATA/bin/soze" | awk '{print $1}')" != "$before" ]
  cmp "$HOME/config.before" "$HOME/.config/soze/config.toml"
  cmp "$HOME/tap.before" "$DATA/tap.jsonl"
  cmp "$HOME/log.before" "$DATA/logs/soze.log.1"
  eq "$(systemctl_calls)" "$(printf '%s\n' 'show --property=UnitPath --value' daemon-reload 'enable soze.service' 'restart soze.service')" "update calls"
}

uses_the_user_managers_config_directory() {
  export XDG_CONFIG_HOME="$HOME/custom config"
  printf '/etc/systemd/user "%s/systemd/user" /usr/lib/systemd/user\n' "$XDG_CONFIG_HOME" > "$HOME/systemctl.unit-paths"
  install_fake

  # Only the supervisor definition follows XDG; Soze keeps its existing config/data contract.
  [ -f "$XDG_CONFIG_HOME/systemd/user/soze.service" ]
  [ ! -e "$UNIT" ]
  [ -f "$HOME/.config/soze/config.toml" ]
  service uninstall
  [ ! -e "$XDG_CONFIG_HOME/systemd/user/soze.service" ]
}

manager_paths_with_escaped_characters_remain_valid() {
  export XDG_CONFIG_HOME="$HOME/custom \"config\\dir \$HOME"
  printf '/etc/systemd/user "%s/custom \\"config\\\\dir \\$HOME/systemd/user"\n' "$HOME" > "$HOME/systemctl.unit-paths"

  install_fake

  # Quoting and escape sequences describe literal path characters, never shell expansions.
  [ -f "$XDG_CONFIG_HOME/systemd/user/soze.service" ]
  [ ! -e "$UNIT" ]
  service uninstall
  [ ! -e "$XDG_CONFIG_HOME/systemd/user/soze.service" ]
}

config_directory_trailing_slashes_remain_valid() {
  export XDG_CONFIG_HOME="$HOME/custom config/"
  printf '"%s/custom config/systemd/user"\n' "$HOME" > "$HOME/systemctl.unit-paths"

  install_fake

  # A trailing XDG slash still addresses the directory searched by the manager.
  [ -f "$XDG_CONFIG_HOME/systemd/user/soze.service" ]
  service uninstall
  [ ! -e "$XDG_CONFIG_HOME/systemd/user/soze.service" ]
}

malformed_manager_paths_cannot_authorize_installation() {
  printf '"%s/.config/systemd/user\n' "$HOME" > "$HOME/systemctl.unit-paths"
  fake_soze "$HOME/soze" one

  code=$(exit_code install "$HOME/soze")

  # A partial quoted entry is not evidence that the manager searches that directory.
  eq "$code" 2 "malformed unit path"
  [ ! -e "$HOME/.local" ]
  [ ! -e "$HOME/.config" ]
}

caller_only_config_override_preserves_the_running_installation() {
  installed
  printf 'synthetic\n' > "$DATA/logs/soze.log"
  printf '{"synthetic":true}\n' > "$DATA/tap.jsonl"
  cp -Rp "$DATA" "$HOME/data.before"
  cp "$UNIT" "$HOME/unit.before"
  cp "$HOME/.config/soze/config.toml" "$HOME/config.before"
  inode_before=$(ls -i "$DATA/bin/soze" | awk '{print $1}')
  export XDG_CONFIG_HOME="$HOME/caller config"
  fake_soze "$HOME/soze" two

  code=$(exit_code install "$HOME/soze")

  # A caller-only directory cannot replace files or disturb the already running service.
  eq "$code" 2 "unsearchable unit directory"
  diff -rq "$HOME/data.before" "$DATA" > /dev/null
  cmp "$HOME/unit.before" "$UNIT"
  cmp "$HOME/config.before" "$HOME/.config/soze/config.toml"
  eq "$(ls -i "$DATA/bin/soze" | awk '{print $1}')" "$inode_before" "binary inode"
  [ -e "$HOME/systemctl.running" ]
  [ ! -e "$XDG_CONFIG_HOME" ]
  eq "$(systemctl_calls)" 'show --property=UnitPath --value' "preflight calls only"
}

manager_path_prefixes_do_not_authorize_a_new_installation() {
  export XDG_CONFIG_HOME="$HOME/caller config"
  printf '"%s/systemd/user-extra" "%s/systemd/user/nested"\n' "$XDG_CONFIG_HOME" "$XDG_CONFIG_HOME" > "$HOME/systemctl.unit-paths"
  fake_soze "$HOME/soze" one

  code=$(exit_code install "$HOME/soze")

  # Only a complete search-path entry authorizes the unit directory, not a substring.
  eq "$code" 2 "unsearchable unit directory"
  [ ! -e "$HOME/.local" ]
  [ ! -e "$HOME/.config" ]
  [ ! -e "$XDG_CONFIG_HOME" ]
}

restart_and_reload_use_the_user_manager() {
  installed

  service restart
  service reload

  # Both operations target only this user's Soze unit.
  eq "$(systemctl_calls)" "$(printf '%s\n' 'restart soze.service' 'reload soze.service')" "control calls"
}

control_failures_propagate() {
  # No unit is available to restart or reload.
  [ "$(exit_code restart)" -ne 0 ]
  [ "$(exit_code reload)" -ne 0 ]
}

status_without_an_install_is_not_loaded() {
  service status > "$HOME/out"

  # An absent unit is distinct from a failed installed unit.
  eq "$(cat "$HOME/out")" "$(printf 'state=not loaded\npid=')" status
}

status_reports_failed_units_without_a_pid() {
  installed
  "$RM" "$HOME/systemctl.running"

  service status > "$HOME/out"

  # A supervisor failure remains visible rather than being reported as not installed.
  grep -qx 'state=failed' "$HOME/out"
  grep -qx 'pid=' "$HOME/out"
}

status_fails_when_the_user_manager_is_unavailable() {
  echo show > "$HOME/systemctl.fail"

  # A missing user bus must not masquerade as an uninstalled service.
  [ "$(exit_code status)" -ne 0 ]
  [ ! -s "$HOME/out" ]
}

logs_follow_only_the_operational_file() {
  fake_tail

  service logs

  # Neither captures nor the journal can leak into the logs command.
  eq "$(cat "$HOME/tail.args")" "$(printf '%s\n' -F "$DATA/logs/soze.log")" "tail arguments"
}

uninstall_disables_the_unit_and_preserves_data() {
  installed
  printf '{"synthetic":true}\n' > "$DATA/tap.jsonl"
  cp "$DATA/tap.jsonl" "$HOME/tap.before"

  service uninstall > "$HOME/out"

  # Stop/disable precedes removal and reload; captures and configuration survive.
  eq "$(systemctl_calls)" "$(printf '%s\n' 'show soze.service --property=FragmentPath --value' 'disable --now soze.service' daemon-reload)" "uninstall calls"
  [ ! -e "$UNIT" ]
  [ ! -e "$DATA/bin" ]
  [ ! -e "$HOME/systemctl.enabled" ]
  [ ! -e "$HOME/systemctl.running" ]
  cmp "$HOME/tap.before" "$DATA/tap.jsonl"
  [ -e "$HOME/.config/soze/config.toml" ]
  [ ! -s "$HOME/out" ]
}

uninstall_is_idempotent() {
  echo show > "$HOME/systemctl.fail"
  service uninstall
  service uninstall

  # No installed unit means no supervisor operation is needed.
  [ ! -e "$HOME/systemctl.calls" ]
  [ ! -e "$HOME/.local" ]
}

rejects_uninstall_without_mutating_the_installation() {
  expected_code=$1
  shift
  printf 'synthetic\n' > "$DATA/logs/soze.log"
  printf '{"synthetic":true}\n' > "$DATA/tap.jsonl"
  cp -Rp "$DATA" "$HOME/data.before"
  cp "$UNIT" "$HOME/unit.before"
  cp "$HOME/.config/soze/config.toml" "$HOME/config.before"
  inode_before=$(ls -i "$DATA/bin/soze" | awk '{print $1}')

  code=$(exit_code uninstall "$@")

  # Identity rejection or a failed query cannot stop/disable or change installed files/data.
  eq "$code" "$expected_code" "uninstall exit status"
  diff -rq "$HOME/data.before" "$DATA" > /dev/null
  cmp "$HOME/unit.before" "$UNIT"
  cmp "$HOME/config.before" "$HOME/.config/soze/config.toml"
  eq "$(ls -i "$DATA/bin/soze" | awk '{print $1}')" "$inode_before" "binary inode"
  [ -e "$HOME/systemctl.running" ]
  [ -e "$HOME/systemctl.enabled" ]
  eq "$(systemctl_calls)" 'show soze.service --property=FragmentPath --value' "identity query only"
  [ ! -s "$HOME/out" ]
}

caller_only_override_cannot_uninstall_the_running_service() {
  installed
  export XDG_CONFIG_HOME="$HOME/caller config"

  rejects_uninstall_without_mutating_the_installation 2

  # An absent caller-only directory must remain absent.
  [ ! -e "$XDG_CONFIG_HOME" ]
}

caller_only_override_cannot_purge_the_running_service() {
  installed
  export XDG_CONFIG_HOME="$HOME/caller config"

  rejects_uninstall_without_mutating_the_installation 2 --purge

  # Purge cannot create or remove an unrelated caller-only directory.
  [ ! -e "$XDG_CONFIG_HOME" ]
}

an_absent_executable_does_not_bypass_the_identity_check() {
  installed
  "$RM" "$DATA/bin/soze"
  export XDG_CONFIG_HOME="$HOME/caller config"
  cp "$DATA/bin/install.json" "$HOME/record.before"

  code=$(exit_code uninstall --purge)

  # Remaining provenance still identifies an installation even if its executable was removed.
  eq "$code" 2 "uninstall exit status"
  cmp "$HOME/record.before" "$DATA/bin/install.json"
  [ -f "$UNIT" ]
  [ -e "$HOME/systemctl.running" ]
  [ -e "$HOME/systemctl.enabled" ]
  eq "$(systemctl_calls)" 'show soze.service --property=FragmentPath --value' "identity query only"
}

a_searched_but_shadowed_definition_cannot_uninstall_the_loaded_unit() {
  installed
  export XDG_CONFIG_HOME="$HOME/other config"
  printf '"%s/.config/systemd/user" "%s/systemd/user"\n' "$HOME" "$XDG_CONFIG_HOME" > "$HOME/systemctl.unit-paths"
  mkdir -p "$XDG_CONFIG_HOME/systemd/user"
  cp "$UNIT" "$XDG_CONFIG_HOME/systemd/user/soze.service"
  cp "$UNIT" "$HOME/other-unit.before"

  rejects_uninstall_without_mutating_the_installation 2

  # Search visibility and an identical definition do not prove which path is loaded.
  cmp "$HOME/other-unit.before" "$XDG_CONFIG_HOME/systemd/user/soze.service"
}

an_empty_fragment_identity_prevents_purging_the_installation() {
  installed
  : > "$HOME/systemctl.fragment-path"

  rejects_uninstall_without_mutating_the_installation 2 --purge
}

an_unavailable_manager_prevents_uninstall_mutation() {
  installed
  echo show > "$HOME/systemctl.fail"

  rejects_uninstall_without_mutating_the_installation 1 --purge
}

purge_deletes_only_the_data_directory() {
  installed
  printf '{"synthetic":true}\n' > "$DATA/tap.jsonl"

  service uninstall --purge > "$HOME/out"

  # Purge lists paths, not capture values, and keeps CA/config storage.
  grep -qxF "$DATA/tap.jsonl" "$HOME/out"
  eq "$(grep -c synthetic "$HOME/out")" 0 "capture content lines"
  [ ! -e "$DATA" ]
  [ ! -e "$UNIT" ]
  [ -e "$HOME/.config/soze/config.toml" ]
}

an_unavailable_manager_prevents_installation() {
  echo show > "$HOME/systemctl.fail"
  fake_soze "$HOME/soze" one

  # Preflight failure leaves the installed files untouched.
  [ "$(exit_code install "$HOME/soze")" -ne 0 ]
  [ ! -e "$HOME/.local" ]
  [ ! -e "$HOME/.config" ]
}

install_failures_propagate() {
  fake_soze "$HOME/soze" one
  for command in daemon-reload enable restart; do
    echo "$command" > "$HOME/systemctl.fail"

    # Failure at any required supervisor step fails the install.
    [ "$(exit_code install "$HOME/soze")" -ne 0 ]
  done
}

a_stop_failure_preserves_the_installation() {
  installed
  echo disable > "$HOME/systemctl.fail"

  # Do not remove or purge files while the supervisor may still own a running process.
  [ "$(exit_code uninstall --purge)" -ne 0 ]
  [ -e "$UNIT" ]
  [ -x "$DATA/bin/soze" ]
  [ -e "$HOME/systemctl.running" ]
}

a_missing_unit_file_does_not_bypass_the_stop_check() {
  installed
  "$RM" "$UNIT"
  echo disable > "$HOME/systemctl.fail"
  printf '{"synthetic":true}\n' > "$DATA/tap.jsonl"

  # A deleted definition does not prove that the manager stopped the installed process.
  [ "$(exit_code uninstall --purge)" -ne 0 ]
  [ -x "$DATA/bin/soze" ]
  [ -e "$DATA/tap.jsonl" ]
  [ -e "$HOME/systemctl.running" ]
  eq "$(systemctl_calls)" "$(printf '%s\n' 'show soze.service --property=FragmentPath --value' 'disable --now soze.service')" "missing definition still requires stop"
}

a_removed_definition_can_be_uninstalled_using_the_cached_identity() {
  installed
  "$RM" "$UNIT"

  service uninstall

  # A cached matching FragmentPath still requires a successful stop before removing the binary.
  [ ! -e "$DATA/bin" ]
  [ ! -e "$HOME/systemctl.running" ]
  [ ! -e "$HOME/systemctl.enabled" ]
  eq "$(systemctl_calls)" "$(printf '%s\n' 'show soze.service --property=FragmentPath --value' 'disable --now soze.service' daemon-reload)" "removed definition uninstall calls"
}

invalid_arguments_change_nothing() {
  fake_soze "$HOME/soze" one

  # Invalid commands/binaries are rejected before contacting either supervisor.
  eq "$(exit_code install "$HOME/missing")" 2 "bad binary"
  eq "$(exit_code restart extra)" 2 "extra argument"
  eq "$(exit_code uninstall --force)" 2 "unknown option"
  [ ! -e "$HOME/systemctl.calls" ]
  [ ! -e "$HOME/launchctl.calls" ]
  [ ! -e "$HOME/.local" ]
}

an_empty_home_changes_nothing() {
  fake_soze "$HOME/soze" one
  code=0

  env HOME= sh "$ROOT/scripts/service.sh" install "$HOME/soze" > /dev/null 2>&1 || code=$?

  # Missing HOME cannot redirect installation into root-owned paths.
  eq "$code" 2 "exit status"
  [ ! -e "$HOME/systemctl.calls" ]
}

an_unsupported_platform_changes_nothing() {
  fake_uname FreeBSD
  fake_soze "$HOME/soze" one

  # Unsupported platforms fail explicitly rather than selecting launchd.
  eq "$(exit_code install "$HOME/soze")" 2 "exit status"
  [ ! -e "$HOME/launchctl.calls" ]
  [ ! -e "$HOME/systemctl.calls" ]
  [ ! -e "$HOME/.local" ]
}

case_run installs_an_enabled_always_on_user_unit installs_an_enabled_always_on_user_unit
case_run shares_config_and_provenance_with_macos shares_config_and_provenance_with_macos
case_run updates_the_binary_preserving_config_and_captures updates_the_binary_preserving_config_and_captures
case_run uses_the_user_managers_config_directory uses_the_user_managers_config_directory
case_run manager_paths_with_escaped_characters_remain_valid manager_paths_with_escaped_characters_remain_valid
case_run config_directory_trailing_slashes_remain_valid config_directory_trailing_slashes_remain_valid
case_run malformed_manager_paths_cannot_authorize_installation malformed_manager_paths_cannot_authorize_installation
case_run caller_only_config_override_preserves_the_running_installation caller_only_config_override_preserves_the_running_installation
case_run manager_path_prefixes_do_not_authorize_a_new_installation manager_path_prefixes_do_not_authorize_a_new_installation
case_run restart_and_reload_use_the_user_manager restart_and_reload_use_the_user_manager
case_run control_failures_propagate control_failures_propagate
case_run status_without_an_install_is_not_loaded status_without_an_install_is_not_loaded
case_run status_reports_failed_units_without_a_pid status_reports_failed_units_without_a_pid
case_run status_fails_when_the_user_manager_is_unavailable status_fails_when_the_user_manager_is_unavailable
case_run logs_follow_only_the_operational_file logs_follow_only_the_operational_file
case_run uninstall_disables_the_unit_and_preserves_data uninstall_disables_the_unit_and_preserves_data
case_run uninstall_is_idempotent uninstall_is_idempotent
case_run caller_only_override_cannot_uninstall_the_running_service caller_only_override_cannot_uninstall_the_running_service
case_run caller_only_override_cannot_purge_the_running_service caller_only_override_cannot_purge_the_running_service
case_run an_absent_executable_does_not_bypass_the_identity_check an_absent_executable_does_not_bypass_the_identity_check
case_run a_searched_but_shadowed_definition_cannot_uninstall_the_loaded_unit a_searched_but_shadowed_definition_cannot_uninstall_the_loaded_unit
case_run an_empty_fragment_identity_prevents_purging_the_installation an_empty_fragment_identity_prevents_purging_the_installation
case_run an_unavailable_manager_prevents_uninstall_mutation an_unavailable_manager_prevents_uninstall_mutation
case_run purge_deletes_only_the_data_directory purge_deletes_only_the_data_directory
case_run an_unavailable_manager_prevents_installation an_unavailable_manager_prevents_installation
case_run install_failures_propagate install_failures_propagate
case_run a_stop_failure_preserves_the_installation a_stop_failure_preserves_the_installation
case_run a_missing_unit_file_does_not_bypass_the_stop_check a_missing_unit_file_does_not_bypass_the_stop_check
case_run a_removed_definition_can_be_uninstalled_using_the_cached_identity a_removed_definition_can_be_uninstalled_using_the_cached_identity
case_run invalid_arguments_change_nothing invalid_arguments_change_nothing
case_run an_empty_home_changes_nothing an_empty_home_changes_nothing
case_run an_unsupported_platform_changes_nothing an_unsupported_platform_changes_nothing
finish
