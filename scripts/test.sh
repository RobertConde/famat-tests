#!/usr/bin/env bash
# Run the HTTP test suite against the local Postgres and Garage from
# scripts/dev-up.sh. Extra arguments go to `cargo test`.
#
#   scripts/test.sh
#   scripts/test.sh search_page    # only tests whose name matches
set -euo pipefail
cd "$(dirname "$0")/.."
exec scripts/with-dev-env.sh cargo test --locked --features server "$@"
