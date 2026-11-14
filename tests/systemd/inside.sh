#!/bin/sh
set -eu

# Some hardware-oriented units may be unavailable in a container; a settled degraded boot is OK.
state=$(timeout 30 systemctl is-system-running --wait) || [ "$state" = degraded ]
systemctl start user@1000.service
exec runuser -u sozetest -- env \
  HOME=/home/sozetest \
  XDG_RUNTIME_DIR=/run/user/1000 \
  DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus \
  python3 /opt/soze/tests/systemd/test_service.py
