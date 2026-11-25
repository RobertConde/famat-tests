#!/usr/bin/env bash
# Run a command with the spike's secrets loaded from outside the repo:
#   $STATE/s3.env     written by garage/up.sh
#   $STATE/admin.env  ADMIN_PASSCODE_HASH='<output of the hash_passcode example>'
# e.g.  ./run.sh dx serve --web
#       ./run.sh target/dx/famat-dioxus-spike/release/web/famat-dioxus-spike
set -euo pipefail
STATE="${STATE:-$HOME/.cache/famat-garage-spike}"
set -a
. "$STATE/s3.env"
. "$STATE/admin.env"
set +a
exec "$@"
