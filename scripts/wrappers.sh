#!/bin/sh
# Installs or removes the soze agent wrappers: one file under ~/.config/soze and one marked block
# in the zsh or bash start-up file. Usage: wrappers.sh install|uninstall [--dry-run]
here=$(cd "$(dirname "$0")" && pwd) || exit 1
action=${1:-}
dry=0
case ${2:-} in
  '') ;;
  --dry-run) dry=1 ;;
  *) echo "unknown argument: ${2}" >&2; exit 2 ;;
esac
case $action in
  install | uninstall) ;;
  *) echo "usage: wrappers.sh install|uninstall [--dry-run]" >&2; exit 2 ;;
esac
case $(basename "${SHELL:-}") in
  zsh) rc=$HOME/.zshrc ;;
  bash) rc=$HOME/.bashrc ;;
  *) echo "unsupported shell: only zsh and bash" >&2; exit 2 ;;
esac

dest=$HOME/.config/soze/wrappers.sh
begin='# >>> soze wrappers >>>'
end='# <<< soze wrappers <<<'
block="$begin
. \"\$HOME/.config/soze/wrappers.sh\"
$end"

has_block() {
  [ -f "$rc" ] && grep -qxF "$begin" "$rc"
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
  has_block && exit 0
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
if [ "$begins" -gt 0 ]; then
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
