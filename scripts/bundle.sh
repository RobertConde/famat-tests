#!/usr/bin/env bash
# Build the release bundle (server binary + public/ with the WASM client) into
# a clean dist/, since stale hashed assets accumulate otherwise (ADR 0001).
#
#   scripts/bundle.sh
#   dist/server  (with dist/public beside it; same environment as scripts/dev.sh)
#
# Needs dioxus-cli 0.7.10: cargo install dioxus-cli --version 0.7.10 --locked
set -euo pipefail
cd "$(dirname "$0")/.."

want="0.7.10"
have="$(dx --version | awk '{print $2}')"
if [ "$have" != "$want" ]; then
  echo "dx $have found; this project needs dioxus-cli $want (ADR 0001)." >&2
  exit 1
fi

rm -rf dist
dx bundle --web --release --out-dir dist
ls -la dist
