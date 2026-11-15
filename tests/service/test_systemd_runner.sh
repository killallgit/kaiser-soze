. "$(dirname "$0")/lib.sh"

runner_shell=sh
[ -z "${BASH_VERSION:-}" ] || runner_shell=bash
[ -z "${ZSH_VERSION:-}" ] || runner_shell=zsh

fake_docker() {
  mkdir -p "$HOME/work"
  export TMPDIR="$HOME/work"
  cat > "$HOME/bin/timeout" <<'FAKE'
#!/bin/sh
printf '%s\n' "$1" >> "$HOME/timeouts"
shift
exec "$@"
FAKE
  cat > "$HOME/bin/docker" <<'FAKE'
#!/bin/sh
command=$1
shift
printf '%s\n' "$@" > "$HOME/docker.$command"
case "$command" in
  context) echo synthetic-context ;;
  build)
    [ ! -e "$HOME/build.fail" ] || exit 23
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --iidfile) printf '%s\n' sha256:synthetic-image > "$2"; shift ;;
      esac
      shift
    done
    ;;
  run)
    cidfile=
    name=
    label=
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --cidfile) cidfile=$2; shift ;;
        --name) name=$2; shift ;;
        --label) label=$2; shift ;;
        --tmpfs) shift ;;
        --detach | --privileged | --cgroupns=private | --network=none) ;;
        -*) exit 2 ;;
        *) printf '%s\n' "$1" > "$HOME/image.used" ;;
      esac
      shift
    done
    printf '%s\n' "$name" > "$HOME/name.used"
    printf '%s\n' "${label#*=}" > "$HOME/owner"
    printf '%s\n' synthetic-container > "$HOME/container"
    if [ -e "$HOME/name.collision" ]; then
      echo foreign-owner > "$HOME/owner"
      echo foreign-container > "$HOME/container"
      exit 125
    fi
    [ ! -e "$HOME/run.fail" ] || exit 41
    if [ -e "$HOME/run.interrupt" ]; then
      kill -TERM "$PPID"
      exit 0
    fi
    printf '%s\n' synthetic-container > "$cidfile"
    ;;
  inspect)
    [ -e "$HOME/container" ] || exit 1
    printf '%s %s\n' "$(cat "$HOME/container")" "$(cat "$HOME/owner")"
    ;;
  exec) [ ! -e "$HOME/exec.fail" ] || exit 37 ;;
  rm)
    [ "$1" = --force ] && [ "$2" = "$(cat "$HOME/container")" ] || exit 2
    [ ! -e "$HOME/remove.fail" ] || exit 17
    rm "$HOME/container"
    ;;
  *) exit 2 ;;
esac
FAKE
  chmod +x "$HOME/bin/docker" "$HOME/bin/timeout"
}

runner_code() {
  code=0
  "$runner_shell" "$ROOT/tests/systemd/run.sh" > "$HOME/out" 2>&1 || code=$?
  printf '%s\n' "$code"
}

launches_the_built_immutable_image_in_isolation() {
  fake_docker

  result=$(runner_code)

  # The launched image is the build's ID, with no shared mutable tag or host access.
  eq "$result" 0 "exit status"
  eq "$(cat "$HOME/image.used")" sha256:synthetic-image "immutable image"
  [ -n "$(cat "$HOME/name.used")" ]
  [ -n "$(cat "$HOME/owner")" ]
  eq "$(cat "$HOME/docker.build")" "$(printf '%s\n' --file "$ROOT/tests/systemd/Dockerfile" --iidfile "$(cat "$HOME/owner")/image.id" "$ROOT")" "build arguments"
  eq "$(cat "$HOME/docker.run")" "$(printf '%s\n' --detach --privileged --cgroupns=private --network=none --tmpfs /run --tmpfs /run/lock --tmpfs /tmp --name "$(cat "$HOME/name.used")" --label "com.killallgit.soze.systemd-test=$(cat "$HOME/owner")" --cidfile "$(cat "$HOME/owner")/container.id" sha256:synthetic-image)" "isolated run arguments"
}

cleans_up_after_successful_exec() {
  fake_docker

  result=$(runner_code)

  # Successful execution removes only its container and temporary build/CID files.
  eq "$result" 0 "exit status"
  eq "$(cat "$HOME/docker.exec")" "$(printf '%s\n' synthetic-container sh /opt/soze/tests/systemd/inside.sh)" "exec arguments"
  eq "$(cat "$HOME/docker.rm")" "$(printf '%s\n' --force synthetic-container)" "removed container"
  eq "$(cat "$HOME/timeouts")" "$(printf '%s\n' 300 30 120 30)" "command bounds"
  [ ! -e "$HOME/container" ]
  [ -z "$(ls -A "$HOME/work")" ]
}

cleans_up_after_failed_exec() {
  fake_docker
  touch "$HOME/exec.fail"

  result=$(runner_code)

  # Cleanup does not replace the command's failing exit status.
  eq "$result" 37 "exit status"
  [ -e "$HOME/docker.rm" ]
  [ ! -e "$HOME/container" ]
  [ -z "$(ls -A "$HOME/work")" ]
}

cleans_up_when_creation_fails_before_delivering_the_cid() {
  fake_docker
  touch "$HOME/run.fail"

  result=$(runner_code)

  # The ownership-checked name fallback removes the created container by ID.
  eq "$result" 41 "exit status"
  eq "$(cat "$HOME/docker.inspect")" "$(printf '%s\n' --format '{{.Id}} {{index .Config.Labels "com.killallgit.soze.systemd-test"}}' "$(cat "$HOME/name.used")")" "ownership inspection"
  eq "$(cat "$HOME/docker.rm")" "$(printf '%s\n' --force synthetic-container)" "removed container"
  [ ! -e "$HOME/docker.exec" ]
  [ ! -e "$HOME/container" ]
  [ -z "$(ls -A "$HOME/work")" ]
}

cleans_up_when_interrupted_before_delivering_the_cid() {
  fake_docker
  touch "$HOME/run.interrupt"

  result=$(runner_code)

  # Termination uses the same ownership-checked fallback and preserves the signal status.
  eq "$result" 143 "exit status"
  eq "$(cat "$HOME/docker.rm")" "$(printf '%s\n' --force synthetic-container)" "removed container"
  [ ! -e "$HOME/container" ]
}

failed_build_never_attempts_container_cleanup() {
  fake_docker
  touch "$HOME/build.fail"
  echo foreign-container > "$HOME/container"

  result=$(runner_code)

  # A failed build cannot authorize inspecting or deleting an unrelated container.
  eq "$result" 23 "exit status"
  [ ! -e "$HOME/docker.run" ]
  [ ! -e "$HOME/docker.inspect" ]
  [ ! -e "$HOME/docker.rm" ]
  eq "$(cat "$HOME/container")" foreign-container "untouched container"
  [ -z "$(ls -A "$HOME/work")" ]
}

name_collision_never_deletes_the_existing_container() {
  fake_docker
  touch "$HOME/name.collision"

  result=$(runner_code)

  # A matching name without this invocation's ownership label is not ours to remove.
  eq "$result" 125 "exit status"
  [ ! -e "$HOME/docker.rm" ]
  eq "$(cat "$HOME/container")" foreign-container "untouched container"
  [ -z "$(ls -A "$HOME/work")" ]
}

cleanup_failure_preserves_an_existing_failure() {
  fake_docker
  touch "$HOME/exec.fail" "$HOME/remove.fail"

  result=$(runner_code)

  # A cleanup failure must not hide the original failing command's exit status.
  eq "$result" 37 "exit status"
  [ -e "$HOME/docker.rm" ]
}

cleanup_failure_fails_an_otherwise_successful_run() {
  fake_docker
  touch "$HOME/remove.fail"

  result=$(runner_code)

  # A leaked container makes an otherwise successful integration run fail.
  eq "$result" 1 "exit status"
  [ -e "$HOME/docker.rm" ]
}

case_run launches_the_built_immutable_image_in_isolation launches_the_built_immutable_image_in_isolation
case_run cleans_up_after_successful_exec cleans_up_after_successful_exec
case_run cleans_up_after_failed_exec cleans_up_after_failed_exec
case_run cleans_up_when_creation_fails_before_delivering_the_cid cleans_up_when_creation_fails_before_delivering_the_cid
case_run cleans_up_when_interrupted_before_delivering_the_cid cleans_up_when_interrupted_before_delivering_the_cid
case_run failed_build_never_attempts_container_cleanup failed_build_never_attempts_container_cleanup
case_run name_collision_never_deletes_the_existing_container name_collision_never_deletes_the_existing_container
case_run cleanup_failure_preserves_an_existing_failure cleanup_failure_preserves_an_existing_failure
case_run cleanup_failure_fails_an_otherwise_successful_run cleanup_failure_fails_an_otherwise_successful_run
finish
