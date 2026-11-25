#!/usr/bin/env bash
# Start a single-node Garage in Podman, create the bucket and key, and write
# the S3 credentials to $STATE/s3.env (outside the repo; never commit it).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
# On macOS the Podman VM only shares $HOME, so state cannot live in /tmp.
STATE="${STATE:-$HOME/.cache/famat-garage-spike}"
NAME=famat-garage-spike
IMAGE=docker.io/dxflrs/garage:v2.6.0
BUCKET=famat-spike

mkdir -p "$STATE"
if [ ! -f "$STATE/garage.toml" ]; then
  sed "s/__RPC_SECRET__/$(openssl rand -hex 32)/" "$HERE/garage.toml.template" > "$STATE/garage.toml"
fi

podman rm -f "$NAME" >/dev/null 2>&1 || true
podman run -d --name "$NAME" -p 3900:3900 \
  -v "$STATE/garage.toml:/etc/garage.toml:ro" \
  -v famat-garage-meta:/var/lib/garage/meta \
  -v famat-garage-data:/var/lib/garage/data \
  "$IMAGE" >/dev/null

g() { podman exec "$NAME" /garage "$@"; }

for _ in $(seq 1 30); do g status >/dev/null 2>&1 && break; sleep 1; done

NODE_ID="$(g node id -q | cut -d@ -f1)"
if g layout show | grep -q "No nodes currently have a role"; then
  g layout assign -z dc1 -c 1G "$NODE_ID"
  g layout apply --version 1
fi

g bucket info "$BUCKET" >/dev/null 2>&1 || g bucket create "$BUCKET"
if ! g key info famat-spike-key >/dev/null 2>&1; then
  g key create famat-spike-key > "$STATE/key.txt"
fi
g bucket allow --read --write --owner "$BUCKET" --key famat-spike-key >/dev/null

KEY_INFO="$(g key info famat-spike-key --show-secret)"
KEY_ID="$(echo "$KEY_INFO" | awk '/Key ID:/ {print $3}')"
SECRET="$(echo "$KEY_INFO" | awk '/Secret key:/ {print $3}')"
cat > "$STATE/s3.env" <<EOF
S3_ENDPOINT=http://127.0.0.1:3900
S3_REGION=garage
S3_BUCKET=$BUCKET
S3_ACCESS_KEY_ID=$KEY_ID
S3_SECRET_ACCESS_KEY=$SECRET
EOF
echo "Garage up; credentials in $STATE/s3.env"
