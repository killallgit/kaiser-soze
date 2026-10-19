. "$(dirname "$0")/lib.sh"

BEGIN='# >>> soze wrappers >>>'
END='# <<< soze wrappers <<<'
BLOCK="$BEGIN
. \"\$HOME/.config/soze/wrappers.sh\"
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
  installer install
  cmp "$ROOT/scripts/soze-wrappers.sh" "$HOME/.config/soze/wrappers.sh"
  { cat "$HOME/original"; printf '%s\n' "$BLOCK"; } > "$HOME/expected"
  cmp "$HOME/expected" "$HOME/.zshrc"
}

installs_into_bashrc_for_bash() {
  SHELL=/bin/bash
  export SHELL
  installer install
  eq "$(grep -cxF "$BEGIN" "$HOME/.bashrc")" 1 block
}

a_second_install_changes_nothing() {
  zsh_home
  installer install
  cp "$HOME/.zshrc" "$HOME/after-one"
  cp "$HOME/.config/soze/wrappers.sh" "$HOME/file-one"
  installer install
  cmp "$HOME/after-one" "$HOME/.zshrc"
  cmp "$HOME/file-one" "$HOME/.config/soze/wrappers.sh"
  cmp "$HOME/original" "$HOME/.zshrc.soze-backup"
}

a_changing_install_keeps_a_backup() {
  zsh_home
  installer install
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
  installer install
  installer uninstall --dry-run > "$HOME/out"
  [ -e "$HOME/.config/soze/wrappers.sh" ]
  eq "$(grep -cxF "$BEGIN" "$HOME/.zshrc")" 1 block
}

uninstall_restores_the_start_up_file() {
  zsh_home
  installer install
  installer uninstall
  cmp "$HOME/original" "$HOME/.zshrc"
  [ ! -e "$HOME/.config/soze/wrappers.sh" ]
}

uninstall_restores_a_file_without_a_final_newline() {
  SHELL=/bin/zsh
  export SHELL
  printf 'export A=1' > "$HOME/.zshrc"
  installer install
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
  installer install
  "$RM" "$HOME/.config/soze/wrappers.sh"
  installer install
  [ -e "$HOME/.config/soze/wrappers.sh" ]
  eq "$(grep -cxF "$BEGIN" "$HOME/.zshrc")" 1 block
}

uninstall_clears_a_file_without_its_block() {
  zsh_home
  installer install
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
  installer install
  installer uninstall
  [ ! -e "$HOME/.claude" ]
  [ ! -e "$HOME/.codex" ]
  [ ! -e "$HOME/.cursor" ]
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
finish
