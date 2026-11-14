#!/bin/sh
# Boots real systemd on the selected Docker daemon; never a production deployment.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
image=kaiser-soze-systemd-test:local
work=$(mktemp -d)

cleanup() {
  code=$?
  trap - EXIT
  if [ -s "$work/container.id" ]; then
    timeout 30 docker rm --force "$(cat "$work/container.id")" > /dev/null || code=1
  fi
  rm -f "$work/container.id"
  rmdir "$work"
  exit "$code"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP

printf 'Systemd integration tests use Docker context: %s\n' "$(docker context show)"
timeout 300 docker build --file "$root/tests/systemd/Dockerfile" --tag "$image" "$root"
# No host paths, sockets or ports are shared. Privilege is for systemd's private cgroup hierarchy.
timeout 30 docker run --detach --privileged --cgroupns=private --network=none \
  --tmpfs /run --tmpfs /run/lock --tmpfs /tmp \
  --cidfile "$work/container.id" "$image" > /dev/null
timeout 120 docker exec "$(cat "$work/container.id")" sh /opt/soze/tests/systemd/inside.sh
