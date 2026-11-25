#!/usr/bin/env bash
# Run a command with the settings written by scripts/dev-up.sh.
#
#   scripts/with-dev-env.sh <command...>
#
# Variables already set in the environment win over dev.env, so CI or a
# developer can point at other services.
set -euo pipefail

STATE="${FAMAT_DEV_STATE:-$HOME/.cache/famat-dev}"
if [ ! -f "$STATE/dev.env" ]; then
  echo "No $STATE/dev.env: run scripts/dev-up.sh first." >&2
  exit 1
fi

while IFS='=' read -r key value; do
  case "$key" in ''|\#*) continue ;; esac
  if [ -z "${!key+x}" ]; then export "$key=$value"; fi
done < "$STATE/dev.env"

exec "$@"
