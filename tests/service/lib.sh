. "$(dirname "$0")/../wrappers/lib.sh"
# Helpers for the service tests: a throwaway HOME with fake supervisors and git on PATH.

new_home() {
  HOME=$(mktemp -d)
  export HOME
  mkdir -p "$HOME/bin"
  PATH="$HOME/bin:$ORIG_PATH"
  unset SOZE_PORT XDG_CONFIG_HOME
  LABEL=com.killallgit.soze
  DOMAIN="gui/$(id -u)"
  DATA="$HOME/.local/share/soze"
  PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
  UNIT="$HOME/.config/systemd/user/soze.service"
  fake_uname "${SERVICE_OS:-Darwin}"
  fake_launchctl
  fake_systemctl
  fake_git
  touch "$HOME/.marker"
}

fake_uname() {
  printf '#!/bin/sh\nprintf "%%s\\n" "%s"\n' "$1" > "$HOME/bin/uname"
  chmod +x "$HOME/bin/uname"
}

# Records calls and models only the properties used by the installer.
fake_systemctl() {
  cat > "$HOME/bin/systemctl" <<'FAKE'
#!/bin/sh
[ "$1" = --user ] || exit 2
shift
echo "$*" >> "$HOME/systemctl.calls"
if [ -e "$HOME/systemctl.fail" ] && grep -qxF "$1" "$HOME/systemctl.fail"; then exit 1; fi
case "$*" in
  'show --property=UnitPath --value')
    if [ -e "$HOME/systemctl.unit-paths" ]; then
      cat "$HOME/systemctl.unit-paths"
    else
      printf '/etc/systemd/user "%s/.config/systemd/user" /usr/lib/systemd/user\n' "$HOME"
    fi
    ;;
  'daemon-reload') ;;
  'enable soze.service') touch "$HOME/systemctl.enabled" ;;
  'restart soze.service')
    [ -e "$HOME/systemctl.enabled" ] || exit 1
    touch "$HOME/systemctl.running"
    ;;
  'reload soze.service') [ -e "$HOME/systemctl.running" ] || exit 1 ;;
  'show soze.service --property=LoadState --property=SubState --property=MainPID')
    if [ -e "$HOME/systemctl.running" ]; then
      printf 'LoadState=loaded\nSubState=running\nMainPID=4242\n'
    elif [ -e "$HOME/systemctl.enabled" ]; then
      printf 'LoadState=loaded\nSubState=failed\nMainPID=0\n'
    else
      printf 'LoadState=not-found\nSubState=dead\nMainPID=0\n'
    fi
    ;;
  'disable --now soze.service')
    rm -f "$HOME/systemctl.enabled" "$HOME/systemctl.running"
    ;;
  *) exit 2 ;;
esac
FAKE
  chmod +x "$HOME/bin/systemctl"
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
