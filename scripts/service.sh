#!/bin/sh
# Runs soze as a launchd user agent from a binary built by the caller.
# Usage: service.sh install <binary> | restart | reload | status | logs | uninstall [--purge]
set -e
root=$(cd "$(dirname "$0")/.." && pwd)
usage="usage: service.sh install <binary> | restart | reload | status | logs | uninstall [--purge]"

label=com.killallgit.soze
domain="gui/$(id -u)"
data=$HOME/.local/share/soze
plist=$HOME/Library/LaunchAgents/$label.plist
config=$HOME/.config/soze/config.toml

# launchd_field <key>: a value from the top level of `launchctl print` on stdin
launchd_field() {
  awk -v key="$1" 'index($0, "\t" key " = ") == 1 { print substr($0, length(key) + 5); exit }'
}

show_status() {
  state="not loaded"
  pid=
  if printed=$(launchctl print "$domain/$label" 2>/dev/null); then
    state=$(printf '%s\n' "$printed" | launchd_field state)
    pid=$(printf '%s\n' "$printed" | launchd_field pid)
  fi
  printf 'state=%s\npid=%s\n' "$state" "$pid"
  [ ! -e "$data/bin/install.json" ] ||
    sed -n '/^  "/{s/,$//;s/^  "\([a-z_]*\)": /\1=/;s/="\(.*\)"$/=\1/;p;}' "$data/bin/install.json"
}

uninstall() {
  launchctl bootout "$domain/$label" 2>/dev/null || true
  rm -f "$plist" "$data/bin/soze" "$data/bin/install.json"
  rmdir "$data/bin" 2>/dev/null || true
}

# an empty HOME would turn the data dir into /.local/share/soze
purge() {
  [ -n "$HOME" ] || { echo "HOME is empty: nothing purged" >&2; exit 2; }
  uninstall
  [ ! -e "$data" ] || find "$data" -mindepth 1
  rm -rf "$data"
}

case "$#:$*" in
  1:restart) exec launchctl kickstart -k "$domain/$label" ;;
  1:reload) exec launchctl kill HUP "$domain/$label" ;;
  1:status) show_status; exit ;;
  1:logs) exec tail -F "$data/logs/soze.log" ;;
  1:uninstall) uninstall; exit ;;
  '2:uninstall --purge') purge; exit ;;
esac

[ "${1:-}" = install ] && [ $# -eq 2 ] || { echo "$usage" >&2; exit 2; }
binary=$2
[ -f "$binary" ] && [ -x "$binary" ] || { echo "not an executable file: $binary" >&2; exit 2; }

in_repo() {
  (cd "$root" && git "$@" 2>/dev/null)
}

write_record() {
  if sha=$(in_repo rev-parse HEAD) && branch=$(in_repo rev-parse --abbrev-ref HEAD) && changes=$(in_repo status --porcelain); then
    branch=$(printf '%s' "$branch" | sed 's/"/\\"/g')
    dirty=false
    [ -z "$changes" ] || dirty=true
  else
    sha=unknown
    branch=unknown
    dirty=false
  fi
  cat > "$data/bin/install.json" <<JSON
{
  "source": "local",
  "sha": "$sha",
  "branch": "$branch",
  "dirty": $dirty,
  "installed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
JSON
}

write_plist() {
  cat > "$plist.new" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>$label</string>
  <key>ProgramArguments</key>
  <array>
    <string>$data/bin/soze</string>
    <string>--port</string>
    <string>48731</string>
    <string>--tap-file</string>
    <string>$data/tap.jsonl</string>
    <string>--shape-file</string>
    <string>$data/shapes.jsonl</string>
    <string>--log-file</string>
    <string>$data/debug.jsonl</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>$data/logs/soze.log</string>
  <key>StandardErrorPath</key>
  <string>$data/logs/soze.log</string>
</dict>
</plist>
PLIST
}

mkdir -p "$data/bin" "$data/logs" "$(dirname "$plist")" "$(dirname "$config")"
# a rename gives the binary a new inode: overwriting a running signed binary can get it killed
cp "$binary" "$data/bin/soze.new"
chmod 755 "$data/bin/soze.new"
mv -f "$data/bin/soze.new" "$data/bin/soze"
write_record
[ -e "$config" ] || printf 'log_format = "human"\nlive = true\n' > "$config"
[ ! -e "$data/logs/soze.log" ] || mv -f "$data/logs/soze.log" "$data/logs/soze.log.1"
write_plist
changed=0
cmp -s "$plist.new" "$plist" || changed=1
mv -f "$plist.new" "$plist"

if launchctl print "$domain/$label" > /dev/null 2>&1; then
  if [ "$changed" = 0 ]; then
    launchctl kickstart -k "$domain/$label"
    exit
  fi
  launchctl bootout "$domain/$label"
fi
launchctl bootstrap "$domain" "$plist"
