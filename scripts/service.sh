#!/bin/sh
# Runs soze under launchd (macOS) or systemd --user (Linux) from a caller-built binary.
# Usage: service.sh install <binary> | restart | reload | status | logs | uninstall [--purge]
set -e
root=$(cd "$(dirname "$0")/.." && pwd)
usage="usage: service.sh install <binary> | restart | reload | status | logs | uninstall [--purge]"

# An empty HOME would put installation and purge paths under /.
[ -n "$HOME" ] || { echo "HOME is empty: nothing changed" >&2; exit 2; }
platform=$(uname -s)
case "$platform" in
  Darwin) ;;
  Linux)
    command -v systemctl >/dev/null 2>&1 ||
      { echo "Linux service management requires systemd --user" >&2; exit 2; }
    ;;
  *) echo "unsupported service platform: $platform" >&2; exit 2 ;;
esac

label=com.killallgit.soze
domain="gui/$(id -u)"
data=$HOME/.local/share/soze
plist=$HOME/Library/LaunchAgents/$label.plist
unit=${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/soze.service
config=$HOME/.config/soze/config.toml

# launchd_field <key>: a value from the top level of `launchctl print` on stdin
launchd_field() {
  awk -v key="$1" 'index($0, "\t" key " = ") == 1 { print substr($0, length(key) + 5); exit }'
}

show_status() {
  state="not loaded"
  pid=
  if [ "$platform" = Linux ]; then
    printed=$(systemctl --user show soze.service --property=LoadState --property=SubState --property=MainPID) || return
    if [ "$(printf '%s\n' "$printed" | sed -n 's/^LoadState=//p')" != not-found ]; then
      state=$(printf '%s\n' "$printed" | sed -n 's/^SubState=//p')
      pid=$(printf '%s\n' "$printed" | sed -n 's/^MainPID=//p')
      [ "$pid" != 0 ] || pid=
    fi
  elif printed=$(launchctl print "$domain/$label" 2>/dev/null); then
    state=$(printf '%s\n' "$printed" | launchd_field state)
    pid=$(printf '%s\n' "$printed" | launchd_field pid)
  fi
  printf 'state=%s\npid=%s\n' "$state" "$pid"
  [ ! -e "$data/bin/install.json" ] ||
    sed -n '/^  "/{s/,$//;s/^  "\([a-z_]*\)": /\1=/;s/="\(.*\)"$/=\1/;p;}' "$data/bin/install.json"
}

installation_exists() {
  [ -e "$unit" ] || [ -e "$data/bin/soze" ] || [ -e "$data/bin/install.json" ]
}

verify_unit_identity() {
  # Search visibility does not establish which same-named definition the manager loaded.
  fragment=$(systemctl --user show soze.service --property=FragmentPath --value)
  expected_unit=$(printf '%s\n' "$unit" | sed 's://*:/:g')
  [ "$fragment" != "$expected_unit" ] || return 0
  if [ "$1" = new ] && [ -z "$fragment" ] && ! installation_exists; then
    # Only a genuinely absent unit may lack an identity before the first write.
    identity=$(systemctl --user show soze.service --property=LoadState --property=MainPID)
    if [ "$(printf '%s\n' "$identity" | sed -n 's/^LoadState=//p')" = not-found ] &&
      [ "$(printf '%s\n' "$identity" | sed -n 's/^MainPID=//p')" = 0 ]; then
      return 0
    fi
  fi
  echo "unit identity does not match the user manager: refusing service operation" >&2
  exit 2
}

uninstall() {
  if [ "$platform" = Linux ]; then
    if installation_exists; then
      verify_unit_identity existing
      # Do not remove files if the manager could not stop the running service.
      systemctl --user disable --now soze.service
      rm -f "$unit"
      systemctl --user daemon-reload
    fi
  else
    launchctl bootout "$domain/$label" 2>/dev/null || true
    rm -f "$plist"
  fi
  rm -f "$data/bin/soze" "$data/bin/install.json"
  rmdir "$data/bin" 2>/dev/null || true
}

purge() {
  uninstall
  [ ! -e "$data" ] || find "$data" -mindepth 1
  rm -rf "$data"
}

case "$#:$*" in
  1:restart)
    [ "$platform" != Linux ] || exec systemctl --user restart soze.service
    exec launchctl kickstart -k "$domain/$label"
    ;;
  1:reload)
    [ "$platform" != Linux ] || exec systemctl --user reload soze.service
    exec launchctl kill HUP "$domain/$label"
    ;;
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

write_unit() {
  # %h is expanded by systemd, not a shell; quoted argv also supports homes containing spaces.
  cat > "$unit.new" <<'UNIT'
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
}

if [ "$platform" = Linux ]; then
  # Query the running manager, not the caller's potentially different XDG environment.
  unit_paths=$(systemctl --user show --property=UnitPath --value)
  # systemctl quotes string-array entries and backslash-escapes special characters.
  # Decode those entries as data, never shell code, before comparing complete paths.
  if ! printf '%s\n' "$unit_paths" | UNIT_DIRECTORY="${unit%/*}" awk '
    function matches_directory(value) {
      gsub(/\/+/, "/", value)
      sub(/\/$/, "", value)
      return value == directory
    }
    BEGIN {
      directory = ENVIRON["UNIT_DIRECTORY"]
      gsub(/\/+/, "/", directory)
      controls = sprintf("%c%c%c%c%c%c%c", 7, 8, 12, 10, 13, 9, 11)
    }
    {
      if (NR != 1) { invalid = 1; next }
      for (i = 1; i <= length($0); i++) {
        c = substr($0, i, 1)
        if (c == "\\") {
          c = substr($0, ++i, 1)
          octal = substr($0, i, 3)
          if (octal ~ /^[0-3][0-7][0-7]$/) {
            path = path sprintf("%c", substr(octal, 1, 1) * 64 + substr(octal, 2, 1) * 8 + substr(octal, 3, 1))
            i += 2
          } else if (c != "" && index("abfnrtv", c)) {
            path = path substr(controls, index("abfnrtv", c), 1)
          } else if (c == "\\" || c == "\"" || c == "$" || c == "`") {
            path = path c
          } else { invalid = 1; break }
        } else if (c == "\"") {
          quoted = !quoted
        } else if (c ~ /[ \t]/ && !quoted) {
          if (matches_directory(path)) found = 1
          path = ""
        } else {
          path = path c
        }
      }
      if (matches_directory(path)) found = 1
    }
    END { exit (invalid || quoted || !found) }
  '; then
    echo "unit directory is outside the user manager search path: nothing changed" >&2
    exit 2
  fi
  verify_unit_identity new
  umask 077
  definition=$unit
else
  definition=$plist
fi
mkdir -p "$data/bin" "$data/logs" "$(dirname "$definition")" "$(dirname "$config")"
# a rename gives the binary a new inode: overwriting a running signed binary can get it killed
cp "$binary" "$data/bin/soze.new"
chmod 755 "$data/bin/soze.new"
mv -f "$data/bin/soze.new" "$data/bin/soze"
write_record
[ -e "$config" ] || printf 'log_format = "human"\nlive = true\n' > "$config"
[ ! -e "$data/logs/soze.log" ] || mv -f "$data/logs/soze.log" "$data/logs/soze.log.1"
if [ "$platform" = Linux ]; then
  write_unit
  mv -f "$unit.new" "$unit"
  systemctl --user daemon-reload
  systemctl --user enable soze.service
  # Reload/enable can change name resolution; never restart a different effective definition.
  verify_unit_identity existing
  exec systemctl --user restart soze.service
fi
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
