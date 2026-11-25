#!/usr/bin/env bash
# Stop local Postgres and Garage.
#
#   scripts/dev-down.sh          stop and remove the containers, keep the data
#   scripts/dev-down.sh --wipe   also delete the data volumes and dev.env
set -euo pipefail

ENGINE="${CONTAINER_ENGINE:-podman}"
STATE="${FAMAT_DEV_STATE:-$HOME/.cache/famat-dev}"

"$ENGINE" rm -f famat-dev-postgres famat-dev-garage >/dev/null 2>&1 || true

if [ "${1:-}" = "--wipe" ]; then
  "$ENGINE" volume rm -f famat-dev-pg famat-dev-garage-meta famat-dev-garage-data >/dev/null 2>&1 || true
  rm -rf "$STATE"
  echo "Containers, volumes and $STATE removed"
else
  echo "Containers removed; data volumes kept"
fi
