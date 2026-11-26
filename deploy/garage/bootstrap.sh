#!/usr/bin/env bash
# Set up the single Garage node: give it a layout role, create the bucket,
# and import the app's key from /etc/famat/secrets/s3.env. Idempotent: run it
# after the first start, after a restore onto empty data directories, or any
# time.
#
#   sudo famat-garage-bootstrap
#
# Settings (environment):
#   GARAGE_CONTAINER  default famat-garage
#   GARAGE_CAPACITY   the node's capacity in the layout, default 500G. Garage
#                     uses it only to balance nodes, but it must be set.
#   GARAGE_ZONE       default dc1
#   S3_BUCKET         default famat
#   FAMAT_SECRETS     default /etc/famat/secrets
set -euo pipefail

CONTAINER="${GARAGE_CONTAINER:-famat-garage}"
CAPACITY="${GARAGE_CAPACITY:-500G}"
ZONE="${GARAGE_ZONE:-dc1}"
BUCKET="${S3_BUCKET:-famat}"
SECRETS="${FAMAT_SECRETS:-/etc/famat/secrets}"
KEY_NAME=famat-app

g() { docker exec -e RUST_LOG=warn "$CONTAINER" /garage "$@"; }
# secret VAR: VAR's value in s3.env (read, not sourced).
secret() { sed -n "s/^$1=//p" "$SECRETS/s3.env"; }

echo "Waiting for Garage in $CONTAINER..."
for _ in $(seq 1 60); do g status >/dev/null 2>&1 && break; sleep 1; done
g status >/dev/null

# Layout: one node with a role. Only the first run assigns and applies.
if g layout show | grep -q "No nodes currently have a role"; then
  node_id="$(g node id -q | cut -d@ -f1)"
  current="$(g layout show | awk '/Current cluster layout version:/ {print $NF}')"
  g layout assign -z "$ZONE" -c "$CAPACITY" "$node_id" >/dev/null
  g layout apply --version "$(( ${current:-0} + 1 ))" >/dev/null
  echo "Layout: node $node_id in $ZONE with $CAPACITY."
else
  echo "Layout: already set."
fi

if g bucket info "$BUCKET" >/dev/null 2>&1; then
  echo "Bucket $BUCKET: exists."
else
  g bucket create "$BUCKET" >/dev/null
  echo "Bucket $BUCKET: created."
fi

key_id="$(secret S3_ACCESS_KEY_ID)"
[ -n "$key_id" ] || { echo "No S3_ACCESS_KEY_ID in $SECRETS/s3.env (run deploy/create-secrets.sh)." >&2; exit 1; }
if g key info "$key_id" >/dev/null 2>&1; then
  echo "Key $key_id: exists."
else
  # Garage wants "GK" + 24 hex digits and a 64-hex-digit secret, which is
  # what deploy/create-secrets.sh generates.
  g key import --yes -n "$KEY_NAME" "$key_id" "$(secret S3_SECRET_ACCESS_KEY)" >/dev/null
  echo "Key $key_id: imported."
fi
g bucket allow --read --write --owner "$BUCKET" --key "$key_id" >/dev/null
echo "Key $key_id may read and write $BUCKET."
