#!/bin/sh
# Installs or removes the soze agent wrappers: one file under ~/.config/soze and one marked block
# in the start-up file. It asks before editing that file and leaves it alone if it cannot be written.
# Usage: wrappers.sh install|uninstall [--dry-run] [--yes] [--rc FILE]   (SOZE_RC names the file too)
here=$(cd "$(dirname "$0")" && pwd) || exit 1
usage="usage: wrappers.sh install|uninstall [--dry-run] [--yes] [--rc FILE]"
action=${1:-}
case $action in
  install | uninstall) shift ;;
  *) echo "$usage" >&2; exit 2 ;;
esac
dry=0
yes=0
rc=${SOZE_RC:-}
while [ $# -gt 0 ]; do
  case $1 in
    --dry-run) dry=1 ;;
    --yes) yes=1 ;;
    --rc)
      [ $# -ge 2 ] || { echo "$usage" >&2; exit 2; }
      rc=$2
      shift
      ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done
if [ -z "$rc" ]; then
  case $(basename "${SHELL:-}") in
    zsh) rc=$HOME/.zshrc ;;
    bash) rc=$HOME/.bashrc ;;
    *) echo "unsupported shell: only zsh and bash, or name a file with --rc FILE or SOZE_RC" >&2; exit 2 ;;
  esac
fi

dest=$HOME/.config/soze/wrappers.sh
begin='# >>> soze wrappers >>>'
end='# <<< soze wrappers <<<'
block="$begin
if [ -r \"\$HOME/.config/soze/wrappers.sh\" ]; then . \"\$HOME/.config/soze/wrappers.sh\"; fi
$end"

has_block() {
  [ -f "$rc" ] && grep -qxF "$begin" "$rc"
}

# Whether this script could edit the start-up file: it exists and is writable, or it can be created.
editable() {
  if [ -e "$rc" ]; then
    [ -w "$rc" ]
  else
    [ -w "$(dirname "$rc")" ]
  fi
}

# Asks before touching the start-up file. Returns 0 for yes (Enter or y), 2 for edit (name another
# file), 1 for no. No input at all, or any other answer, is no.
confirm() {
  [ "$yes" = 1 ] && return 0
  printf 'Add the soze block to %s? [Y/n/e(dit)] ' "$rc" >&2
  read -r answer || return 1
  case $answer in
    '' | y | Y | yes | YES) return 0 ;;
    e | E | edit) return 2 ;;
    *) return 1 ;;
  esac
}

# Reads the start-up file to use instead; an empty answer keeps the current one.
choose_file() {
  printf 'Start-up file to edit instead: ' >&2
  read -r file || return 0
  case $file in
    '~/'*) file=$HOME/${file#'~/'} ;;
  esac
  [ -z "$file" ] || rc=$file
}

# Says what to do by hand when the start-up file is left alone.
by_hand() {
  printf '%s\n' "$1"
  printf '%s\n' "$2"
  printf 'Another file can be named with --rc FILE or SOZE_RC.\n'
}

backup() {
  [ -f "$rc" ] && cp -p "$rc" "$rc.soze-backup"
  return 0
}

if [ "$action" = install ]; then
  if [ "$dry" = 1 ]; then
    echo "would copy the wrappers file to $dest"
    has_block || printf 'would append to %s:\n%s\n' "$rc" "$block"
    exit 0
  fi
  mkdir -p "$(dirname "$dest")" || exit 1
  cmp -s "$here/soze-wrappers.sh" "$dest" || cp "$here/soze-wrappers.sh" "$dest" || exit 1
  while :; do
    has_block && exit 0
    if ! editable; then
      by_hand "Not editing $rc: it cannot be written. Add this to it, or to a start-up file you can edit, then open a new shell:" "$block"
      exit 0
    fi
    confirm
    case $? in
      0) break ;;
      2) choose_file ;;
      *)
        by_hand "Left $rc alone. To finish by hand, add this to it, then open a new shell:" "$block"
        exit 0
        ;;
    esac
  done
  backup
  # a file without a final newline gets one, so the block starts on its own line
  [ ! -s "$rc" ] || [ -z "$(tail -c 1 "$rc")" ] || echo >> "$rc"
  printf '%s\n' "$block" >> "$rc"
  exit 0
fi

# uninstall
begins=0
ends=0
if [ -f "$rc" ]; then
  begins=$(grep -cxF "$begin" "$rc")
  ends=$(grep -cxF "$end" "$rc")
fi
if [ "$begins" -ne "$ends" ]; then
  echo "unbalanced soze markers in $rc: nothing changed" >&2
  exit 1
fi
if [ "$dry" = 1 ]; then
  [ "$begins" -eq 0 ] || echo "would remove the soze block from $rc"
  [ ! -e "$dest" ] || echo "would remove $dest"
  exit 0
fi
if [ "$begins" -gt 0 ] && ! editable; then
  by_hand "Not editing $rc: it cannot be written. Delete the lines from $begin to $end in it." ""
elif [ "$begins" -gt 0 ]; then
  backup
  tmp=$(mktemp) || exit 1
  awk -v b="$begin" -v e="$end" '$0 == b { skip = 1; next } skip && $0 == e { skip = 0; next } !skip' "$rc" > "$tmp" &&
    cat "$tmp" > "$rc"
  status=$?
  rm -f "$tmp"
  [ "$status" -eq 0 ] || exit "$status"
fi
rm -f "$dest"
rmdir "$(dirname "$dest")" 2>/dev/null
exit 0
