#!/bin/sh
# Boots real systemd on the selected Docker daemon; never a production deployment.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d)
name="kaiser-soze-systemd-${work##*/}"
container_started=false

cleanup() {
  code=$?
  trap - EXIT
  container_id=
  if [ -s "$work/container.id" ]; then
    container_id=$(cat "$work/container.id")
  elif [ "$container_started" = true ]; then
    # Creation may succeed before Docker delivers the CID; a name alone is not proof of ownership.
    container=$(timeout 30 docker inspect --format \
      '{{.Id}} {{index .Config.Labels "com.killallgit.soze.systemd-test"}}' "$name" 2>/dev/null) || container=
    case "$container" in
      *" $work") container_id=${container%" $work"} ;;
    esac
  fi
  if [ -n "$container_id" ]; then
    timeout 30 docker rm --force "$container_id" > /dev/null || {
      [ "$code" -ne 0 ] || code=1
    }
  fi
  rm -f "$work/container.id" "$work/image.id"
  rmdir "$work"
  exit "$code"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP

printf 'Systemd integration tests use Docker context: %s\n' "$(docker context show)"
timeout 300 docker build --file "$root/tests/systemd/Dockerfile" --iidfile "$work/image.id" "$root"
image=$(cat "$work/image.id")
# No host paths, sockets or ports are shared. Privilege is for systemd's private cgroup hierarchy.
container_started=true
timeout 30 docker run --detach --privileged --cgroupns=private --network=none \
  --tmpfs /run --tmpfs /run/lock --tmpfs /tmp \
  --name "$name" --label "com.killallgit.soze.systemd-test=$work" \
  --cidfile "$work/container.id" "$image" > /dev/null
timeout 120 docker exec "$(cat "$work/container.id")" sh /opt/soze/tests/systemd/inside.sh
