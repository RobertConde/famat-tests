#!/usr/bin/env bash
# Install (or update) the FAMAT deployment files on the server from this
# checkout. Safe to re-run: it only copies files and reloads systemd, and
# never touches secrets, data, compose.override.yaml or the backup settings
# once they exist. It starts nothing.
#
#   sudo deploy/install.sh
#
#   /etc/famat/compose.yaml    the Compose project (deploy/compose.yaml)
#   /etc/famat/garage.toml     Garage's configuration
#   /etc/famat/backup.conf     backup settings, from backup.conf.example the
#                              first time only (then edit it)
#   /usr/local/sbin/           famat, famat-update, famat-backup,
#                              famat-garage-bootstrap
#   /etc/systemd/system/       famat.service, famat-update.{service,timer},
#                              famat-backup.{service,timer}
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "Run as root (sudo)." >&2
  exit 1
fi

here="$(cd "$(dirname "$0")" && pwd)"

install -d -m 0755 /etc/famat
install -m 0644 "$here/compose.yaml" /etc/famat/compose.yaml
install -m 0644 "$here/garage/garage.toml" /etc/famat/garage.toml

install -m 0755 "$here/bin/famat" /usr/local/sbin/famat
install -m 0755 "$here/bin/famat-update" /usr/local/sbin/famat-update
install -m 0755 "$here/backup/famat-backup" /usr/local/sbin/famat-backup
install -m 0755 "$here/garage/bootstrap.sh" /usr/local/sbin/famat-garage-bootstrap
install -m 0644 "$here"/systemd/famat.service "$here"/systemd/famat-update.service \
  "$here"/systemd/famat-update.timer \
  "$here/backup/famat-backup.service" "$here/backup/famat-backup.timer" /etc/systemd/system/

if [ ! -e /etc/famat/backup.conf ]; then
  install -m 0600 "$here/backup/backup.conf.example" /etc/famat/backup.conf
  echo "Created /etc/famat/backup.conf: edit it before the first backup."
fi

systemctl daemon-reload
echo "Installed /etc/famat/compose.yaml and the famat units."
if [ -d /etc/famat/secrets ]; then
  # Checks the merged compose files; prints nothing (the output has secrets).
  (cd /etc/famat && docker compose config --quiet) && echo "docker compose config: OK"
else
  echo "Next: deploy/create-secrets.sh (docs/deploy/server-setup.md, step 4)."
fi
