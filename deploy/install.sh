#!/usr/bin/env bash
# Install (or update) the FAMAT deployment files on the server from this
# checkout. Safe to re-run: it only copies files and reloads systemd, and
# never touches secrets, data or the backup settings once they exist.
#
#   sudo deploy/install.sh
#
#   /etc/containers/systemd/   the Quadlet units (deploy/quadlet/)
#   /etc/famat/garage.toml     Garage's configuration
#   /etc/famat/backup.conf     backup settings, from backup.conf.example the
#                              first time only (then edit it)
#   /usr/local/sbin/           famat, famat-backup, famat-garage-bootstrap
#   /etc/systemd/system/       famat-backup.service and .timer
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "Run as root (sudo)." >&2
  exit 1
fi

here="$(cd "$(dirname "$0")" && pwd)"
QUADLET_DIR="${QUADLET_DIR:-/etc/containers/systemd}"

install -d -m 0755 "$QUADLET_DIR" /etc/famat
install -m 0644 "$here"/quadlet/* "$QUADLET_DIR"/
install -m 0644 "$here/garage/garage.toml" /etc/famat/garage.toml

install -m 0755 "$here/bin/famat" /usr/local/sbin/famat
install -m 0755 "$here/backup/famat-backup" /usr/local/sbin/famat-backup
install -m 0755 "$here/garage/bootstrap.sh" /usr/local/sbin/famat-garage-bootstrap
install -m 0644 "$here/backup/famat-backup.service" "$here/backup/famat-backup.timer" /etc/systemd/system/

if [ ! -e /etc/famat/backup.conf ]; then
  install -m 0600 "$here/backup/backup.conf.example" /etc/famat/backup.conf
  echo "Created /etc/famat/backup.conf: edit it before the first backup."
fi

systemctl daemon-reload
echo "Installed. Quadlet units: $(cd "$QUADLET_DIR" && ls famat* | tr '\n' ' ')"
