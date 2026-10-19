. "$(dirname "$0")/lib.sh"

BEGIN='# >>> soze wrappers >>>'
END='# <<< soze wrappers <<<'
BLOCK="$BEGIN
if [ -r \"\$HOME/.config/soze/wrappers.sh\" ]; then . \"\$HOME/.config/soze/wrappers.sh\"; fi
$END"

installer() {
  sh "$ROOT/scripts/wrappers.sh" "$@"
}

# a zsh user with a start-up file
zsh_home() {
  SHELL=/bin/zsh
  export SHELL
  printf 'export A=1\n' > "$HOME/.zshrc"
  cp "$HOME/.zshrc" "$HOME/original"
}

installs_the_file_and_one_block() {
  zsh_home
  installer install --yes
  cmp "$ROOT/scripts/soze-wrappers.sh" "$HOME/.config/soze/wrappers.sh"
  { cat "$HOME/original"; printf '%s\n' "$BLOCK"; } > "$HOME/expected"
  cmp "$HOME/expected" "$HOME/.zshrc"
}

installs_into_bashrc_for_bash() {
  SHELL=/bin/bash
  export SHELL
  installer install --yes
  eq "$(grep -cxF "$BEGIN" "$HOME/.bashrc")" 1 block
}

a_second_install_changes_nothing() {
  zsh_home
  installer install --yes
  cp "$HOME/.zshrc" "$HOME/after-one"
  cp "$HOME/.config/soze/wrappers.sh" "$HOME/file-one"
  installer install --yes
  cmp "$HOME/after-one" "$HOME/.zshrc"
  cmp "$HOME/file-one" "$HOME/.config/soze/wrappers.sh"
  cmp "$HOME/original" "$HOME/.zshrc.soze-backup"
}

a_changing_install_keeps_a_backup() {
  zsh_home
  installer install --yes
  cmp "$HOME/original" "$HOME/.zshrc.soze-backup"
}

dry_run_writes_nothing() {
  zsh_home
  installer install --dry-run > "$HOME/out"
  [ ! -e "$HOME/.config/soze/wrappers.sh" ]
  [ ! -e "$HOME/.zshrc.soze-backup" ]
  cmp "$HOME/original" "$HOME/.zshrc"
  eq "$(grep -c 'export A=1' "$HOME/out")" 0 content
  grep -qF "$BEGIN" "$HOME/out"
  installer install --yes
  installer uninstall --dry-run > "$HOME/out"
  [ -e "$HOME/.config/soze/wrappers.sh" ]
  eq "$(grep -cxF "$BEGIN" "$HOME/.zshrc")" 1 block
}

uninstall_restores_the_start_up_file() {
  zsh_home
  installer install --yes
  installer uninstall
  cmp "$HOME/original" "$HOME/.zshrc"
  [ ! -e "$HOME/.config/soze/wrappers.sh" ]
}

uninstall_restores_a_file_without_a_final_newline() {
  SHELL=/bin/zsh
  export SHELL
  printf 'export A=1' > "$HOME/.zshrc"
  installer install --yes
  installer uninstall
  eq "$(cat "$HOME/.zshrc")" "export A=1" content
}

uninstall_with_nothing_installed_changes_nothing() {
  zsh_home
  installer uninstall
  cmp "$HOME/original" "$HOME/.zshrc"
  [ ! -e "$HOME/.zshrc.soze-backup" ]
}

install_repairs_a_block_without_its_file() {
  zsh_home
  installer install --yes
  "$RM" "$HOME/.config/soze/wrappers.sh"
  installer install --yes
  [ -e "$HOME/.config/soze/wrappers.sh" ]
  eq "$(grep -cxF "$BEGIN" "$HOME/.zshrc")" 1 block
}

uninstall_clears_a_file_without_its_block() {
  zsh_home
  installer install --yes
  cp "$HOME/original" "$HOME/.zshrc"
  installer uninstall
  [ ! -e "$HOME/.config/soze/wrappers.sh" ]
  cmp "$HOME/original" "$HOME/.zshrc"
}

unknown_input_fails_and_changes_nothing() {
  zsh_home
  code=0
  installer install --bogus 2>/dev/null || code=$?
  [ "$code" -ne 0 ]
  code=0
  installer frobnicate 2>/dev/null || code=$?
  [ "$code" -ne 0 ]
  SHELL=/usr/bin/fish
  code=0
  installer install 2>/dev/null || code=$?
  [ "$code" -ne 0 ]
  [ ! -e "$HOME/.config/soze/wrappers.sh" ]
  cmp "$HOME/original" "$HOME/.zshrc"
}

unbalanced_markers_are_left_alone() {
  SHELL=/bin/zsh
  export SHELL
  printf 'export A=1\n%s\nexport B=2\n' "$BEGIN" > "$HOME/.zshrc"
  cp "$HOME/.zshrc" "$HOME/original"
  code=0
  installer uninstall 2>/dev/null || code=$?
  [ "$code" -ne 0 ]
  cmp "$HOME/original" "$HOME/.zshrc"
}

nothing_is_written_under_agent_folders() {
  zsh_home
  installer install --yes
  installer uninstall
  [ ! -e "$HOME/.claude" ]
  [ ! -e "$HOME/.codex" ]
  [ ! -e "$HOME/.cursor" ]
}

asked_but_not_answered_nothing_is_edited_and_the_block_is_printed() {
  zsh_home
  installer install > "$HOME/out" 2>/dev/null < /dev/null
  [ -e "$HOME/.config/soze/wrappers.sh" ]
  cmp "$HOME/original" "$HOME/.zshrc"
  [ ! -e "$HOME/.zshrc.soze-backup" ]
  grep -qF "$BEGIN" "$HOME/out"
  grep -qF "$HOME/.zshrc" "$HOME/out"
}

a_yes_answer_edits_and_any_other_answer_does_not() {
  zsh_home
  printf 'n\n' | installer install > /dev/null 2>/dev/null
  cmp "$HOME/original" "$HOME/.zshrc"
  printf '\n' | installer install > /dev/null 2>/dev/null
  cmp "$HOME/original" "$HOME/.zshrc"
  printf 'y\n' | installer install > /dev/null 2>/dev/null
  eq "$(grep -cxF "$BEGIN" "$HOME/.zshrc")" 1 block
}

a_read_only_start_up_file_is_not_touched_and_the_block_is_printed() {
  [ "$(id -u)" -ne 0 ] || return 0
  zsh_home
  chmod 444 "$HOME/.zshrc"
  code=0
  installer install --yes > "$HOME/out" || code=$?
  eq "$code" 0 status
  cmp "$HOME/original" "$HOME/.zshrc"
  [ ! -e "$HOME/.zshrc.soze-backup" ]
  [ -e "$HOME/.config/soze/wrappers.sh" ]
  grep -qF "$BEGIN" "$HOME/out"
  grep -qF -e "--rc" "$HOME/out"
}

uninstall_with_a_read_only_start_up_file_removes_the_file_and_says_what_to_delete() {
  [ "$(id -u)" -ne 0 ] || return 0
  zsh_home
  installer install --yes
  cp "$HOME/.zshrc" "$HOME/installed"
  chmod 444 "$HOME/.zshrc"
  installer uninstall > "$HOME/out"
  cmp "$HOME/installed" "$HOME/.zshrc"
  [ ! -e "$HOME/.config/soze/wrappers.sh" ]
  grep -qF "$BEGIN" "$HOME/out"
}

rc_option_and_environment_choose_the_start_up_file() {
  SHELL=/usr/bin/fish
  export SHELL
  installer install --yes --rc "$HOME/mine"
  eq "$(grep -cxF "$BEGIN" "$HOME/mine")" 1 block
  SOZE_RC="$HOME/theirs"
  export SOZE_RC
  installer install --yes
  eq "$(grep -cxF "$BEGIN" "$HOME/theirs")" 1 block
  installer install --yes --rc "$HOME/third"
  eq "$(grep -cxF "$BEGIN" "$HOME/third")" 1 block
  installer uninstall --rc "$HOME/mine"
  eq "$(grep -cxF "$BEGIN" "$HOME/mine")" 0 block
}

the_block_is_silent_when_the_wrappers_file_is_gone() {
  out=$(sh -c "$BLOCK" 2>&1)
  eq "$out" "" output
}

case_run installs_the_file_and_one_block installs_the_file_and_one_block
case_run installs_into_bashrc_for_bash installs_into_bashrc_for_bash
case_run a_second_install_changes_nothing a_second_install_changes_nothing
case_run a_changing_install_keeps_a_backup a_changing_install_keeps_a_backup
case_run dry_run_writes_nothing dry_run_writes_nothing
case_run uninstall_restores_the_start_up_file uninstall_restores_the_start_up_file
case_run uninstall_restores_a_file_without_a_final_newline uninstall_restores_a_file_without_a_final_newline
case_run uninstall_with_nothing_installed_changes_nothing uninstall_with_nothing_installed_changes_nothing
case_run install_repairs_a_block_without_its_file install_repairs_a_block_without_its_file
case_run uninstall_clears_a_file_without_its_block uninstall_clears_a_file_without_its_block
case_run unknown_input_fails_and_changes_nothing unknown_input_fails_and_changes_nothing
case_run unbalanced_markers_are_left_alone unbalanced_markers_are_left_alone
case_run nothing_is_written_under_agent_folders nothing_is_written_under_agent_folders
case_run asked_but_not_answered_nothing_is_edited_and_the_block_is_printed asked_but_not_answered_nothing_is_edited_and_the_block_is_printed
case_run a_yes_answer_edits_and_any_other_answer_does_not a_yes_answer_edits_and_any_other_answer_does_not
case_run a_read_only_start_up_file_is_not_touched_and_the_block_is_printed a_read_only_start_up_file_is_not_touched_and_the_block_is_printed
case_run uninstall_with_a_read_only_start_up_file_removes_the_file_and_says_what_to_delete uninstall_with_a_read_only_start_up_file_removes_the_file_and_says_what_to_delete
case_run rc_option_and_environment_choose_the_start_up_file rc_option_and_environment_choose_the_start_up_file
case_run the_block_is_silent_when_the_wrappers_file_is_gone the_block_is_silent_when_the_wrappers_file_is_gone
finish
