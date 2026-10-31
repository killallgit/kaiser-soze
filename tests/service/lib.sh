. "$(dirname "$0")/../wrappers/lib.sh"
# Helpers for the service tests: a throwaway HOME with a fake launchctl and a fake git on PATH.

new_home() {
  HOME=$(mktemp -d)
  export HOME
  mkdir -p "$HOME/bin"
  PATH="$HOME/bin:$ORIG_PATH"
  unset SOZE_PORT
  LABEL=com.killallgit.soze
  DOMAIN="gui/$(id -u)"
  DATA="$HOME/.local/share/soze"
  PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
  fake_launchctl
  fake_git
  touch "$HOME/.marker"
}

# Records every call; the agent is loaded while $HOME/launchctl.loaded exists, and a subcommand
# named in $HOME/launchctl.fail exits 1.
fake_launchctl() {
  cat > "$HOME/bin/launchctl" <<'FAKE'
#!/bin/sh
echo "$*" >> "$HOME/launchctl.calls"
if [ -e "$HOME/launchctl.fail" ] && grep -qxF "$1" "$HOME/launchctl.fail"; then exit 1; fi
loaded=$HOME/launchctl.loaded
case $1 in
  print)
    [ -e "$loaded" ] || exit 113
    printf 'com.killallgit.soze = {\n\tstate = running\n\tpid = 4242\n}\n'
    ;;
  bootstrap) touch "$loaded" ;;
  bootout)
    [ -e "$loaded" ] || exit 3
    rm -f "$loaded"
    ;;
  kickstart | kill) [ -e "$loaded" ] || exit 113 ;;
esac
exit 0
FAKE
  chmod +x "$HOME/bin/launchctl"
}

# The tree is dirty while $HOME/git.dirty exists; the branch is $HOME/git.branch, or main.
fake_git() {
  cat > "$HOME/bin/git" <<'FAKE'
#!/bin/sh
case "$*" in
  'rev-parse HEAD') echo 0123456789abcdef0123456789abcdef01234567 ;;
  'rev-parse --abbrev-ref HEAD') if [ -e "$HOME/git.branch" ]; then cat "$HOME/git.branch"; else echo main; fi ;;
  'status --porcelain') [ ! -e "$HOME/git.dirty" ] || echo ' M README.md' ;;
  *) exit 1 ;;
esac
FAKE
  chmod +x "$HOME/bin/git"
}

fake_tail() {
  cat > "$HOME/bin/tail" <<'FAKE'
#!/bin/sh
printf '%s\n' "$@" > "$HOME/tail.args"
FAKE
  chmod +x "$HOME/bin/tail"
}

service() {
  sh "$ROOT/scripts/service.sh" "$@"
}

# fake_soze <file> <tag>: an executable that differs from another fake only by its tag
fake_soze() {
  printf '#!/bin/sh\n# %s\n' "$2" > "$1"
  chmod +x "$1"
}

loaded() {
  touch "$HOME/launchctl.loaded"
}

calls() {
  cat "$HOME/launchctl.calls"
}

expected_plist() {
  cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.killallgit.soze</string>
  <key>ProgramArguments</key>
  <array>
    <string>$DATA/bin/soze</string>
    <string>--port</string>
    <string>48731</string>
    <string>--tap-file</string>
    <string>$DATA/tap.jsonl</string>
    <string>--shape-file</string>
    <string>$DATA/shapes.jsonl</string>
    <string>--log-file</string>
    <string>$DATA/debug.jsonl</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>$DATA/logs/soze.log</string>
  <key>StandardErrorPath</key>
  <string>$DATA/logs/soze.log</string>
</dict>
</plist>
PLIST
}
