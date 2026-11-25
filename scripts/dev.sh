#!/usr/bin/env bash
# Run the app against the local Postgres and Garage from scripts/dev-up.sh,
# with dx's dev server (rebuilds on change). Extra arguments go to `dx serve`.
#
#   scripts/dev.sh                 http://127.0.0.1:8080
#   scripts/dev.sh --port 9000
set -euo pipefail
cd "$(dirname "$0")/.."
exec scripts/with-dev-env.sh dx serve "$@"
