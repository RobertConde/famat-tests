#!/usr/bin/env bash
# Install (or update) the FAMAT deployment files on the server from this
# checkout. Safe to re-run: it only copies files and reloads systemd, and
# never touches secrets, data, compose.override.yaml or the backup settings
# once they exist. It starts nothing, except to restart a stack it stopped
# while moving an old /etc/famat install (below).
#
#   sudo deploy/install.sh                          # installs to /opt/stacks/famat
#   sudo FAMAT_DIR=/srv/famat deploy/install.sh     # or anywhere else
#
#   $FAMAT_DIR/compose.yaml    the Compose project (deploy/compose.yaml)
#   $FAMAT_DIR/garage.toml     Garage's configuration
#   $FAMAT_DIR/backup.conf     backup settings, from backup.conf.example the
#                              first time only (then edit it)
#   $FAMAT_DIR/secrets/        made by deploy/create-secrets.sh
#   /usr/local/sbin/           famat, famat-update, famat-backup,
#                              famat-garage-bootstrap
#   /etc/systemd/system/       famat.service, famat-update.{service,timer},
#                              famat-backup.{service,timer}
#
# Moving from /etc/famat: earlier versions installed to /etc/famat. If that
# directory exists and $FAMAT_DIR has no secrets yet, its contents (secrets,
# backup.conf, restic-password, compose.override.yaml) are copied into
# $FAMAT_DIR with their permissions, and /etc/famat is renamed to
# /etc/famat.moved-<date> (delete it once everything works). A running stack
# is stopped first and started again afterwards.
set -euo pipefail

if [ "$(id -u)" -ne 0 ]; then
  echo "Run as root (sudo)." >&2
  exit 1
fi

here="$(cd "$(dirname "$0")" && pwd)"
DIR="${FAMAT_DIR:-/opt/stacks/famat}"
DIR="${DIR%/}"
OLD=/etc/famat
DEFAULT=/opt/stacks/famat

restart=0
if [ "$DIR" != "$OLD" ] && [ -d "$OLD" ] && [ ! -e "$DIR/secrets" ]; then
  echo "Moving the install from $OLD to $DIR."
  if systemctl is-active --quiet famat.service; then
    systemctl stop famat.service
    restart=1
  fi
  install -d -m 0755 "$DIR"
  cp -a "$OLD/." "$DIR/"
  # Settings written for the old place still point at it.
  for f in "$DIR/backup.conf" "$DIR/compose.override.yaml"; do
    if [ -f "$f" ]; then sed -i "s#$OLD#$DIR#g" "$f"; fi
  done
  moved="$OLD.moved-$(date +%Y%m%d-%H%M%S)"
  mv "$OLD" "$moved"
  echo "Copied into $DIR. The old directory is $moved; delete it once the stack runs."
fi

# Installs a file, pointing its default paths at $DIR when that isn't the
# default. Usage: put MODE SOURCE DEST
put() {
  if [ "$DIR" = "$DEFAULT" ]; then
    install -m "$1" "$2" "$3"
  else
    local tmp
    tmp="$(mktemp)"
    sed "s#$DEFAULT#$DIR#g" "$2" >"$tmp"
    install -m "$1" "$tmp" "$3"
    rm -f "$tmp"
  fi
}

install -d -m 0755 "$DIR"
put 0644 "$here/compose.yaml" "$DIR/compose.yaml"
put 0644 "$here/garage/garage.toml" "$DIR/garage.toml"

put 0755 "$here/bin/famat" /usr/local/sbin/famat
put 0755 "$here/bin/famat-update" /usr/local/sbin/famat-update
put 0755 "$here/backup/famat-backup" /usr/local/sbin/famat-backup
put 0755 "$here/garage/bootstrap.sh" /usr/local/sbin/famat-garage-bootstrap
for unit in "$here/systemd/famat.service" "$here/systemd/famat-update.service" \
  "$here/systemd/famat-update.timer" \
  "$here/backup/famat-backup.service" "$here/backup/famat-backup.timer"; do
  put 0644 "$unit" "/etc/systemd/system/$(basename "$unit")"
done

if [ ! -e "$DIR/backup.conf" ]; then
  put 0600 "$here/backup/backup.conf.example" "$DIR/backup.conf"
  echo "Created $DIR/backup.conf: edit it before the first backup."
fi

systemctl daemon-reload
echo "Installed $DIR/compose.yaml and the famat units."
if [ -d "$DIR/secrets" ]; then
  # Checks the merged compose files; prints nothing (the output has secrets).
  (cd "$DIR" && docker compose config --quiet) && echo "docker compose config: OK"
else
  echo "Next: FAMAT_SECRETS=$DIR/secrets deploy/create-secrets.sh (docs/deploy/server-setup.md, step 4)."
fi

if [ "$restart" = 1 ]; then
  systemctl start famat.service
  echo "Started famat again."
fi
