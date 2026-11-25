#!/usr/bin/env bash
# Start local Postgres and Garage for development and the HTTP tests.
#
#   scripts/dev-up.sh
#
# Writes the connection settings (with generated secrets) to
# $FAMAT_DEV_STATE/dev.env, outside the repo. Never commit that file.
# Safe to re-run: containers are recreated, data volumes are kept.
#
# Settings (environment):
#   CONTAINER_ENGINE  podman (default) or docker
#   FAMAT_DEV_STATE   state directory (default ~/.cache/famat-dev). On macOS
#                     the Podman VM only shares $HOME, so keep it under $HOME.
#   FAMAT_PG_PORT     host port for Postgres (default 55432)
#   FAMAT_S3_PORT     host port for Garage's S3 API (default 3900)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENGINE="${CONTAINER_ENGINE:-podman}"
STATE="${FAMAT_DEV_STATE:-$HOME/.cache/famat-dev}"
PG_PORT="${FAMAT_PG_PORT:-55432}"
S3_PORT="${FAMAT_S3_PORT:-3900}"

PG_NAME=famat-dev-postgres
PG_IMAGE=docker.io/library/postgres:17-alpine
GARAGE_NAME=famat-dev-garage
GARAGE_IMAGE=docker.io/dxflrs/garage:v2.6.0
BUCKET=famat
KEY_NAME=famat-dev

mkdir -p "$STATE"
chmod 700 "$STATE"

# ------------------------------------------------------------------ Postgres

if [ ! -f "$STATE/pg-password" ]; then
  if "$ENGINE" volume inspect famat-dev-pg >/dev/null 2>&1; then
    echo "The famat-dev-pg volume exists but $STATE/pg-password is gone." >&2
    echo "Run scripts/dev-down.sh --wipe to start over." >&2
    exit 1
  fi
  openssl rand -hex 24 > "$STATE/pg-password"
fi
PG_PASSWORD="$(cat "$STATE/pg-password")"

"$ENGINE" rm -f "$PG_NAME" >/dev/null 2>&1 || true
"$ENGINE" run -d --name "$PG_NAME" \
  -p "127.0.0.1:$PG_PORT:5432" \
  -e POSTGRES_USER=famat \
  -e POSTGRES_PASSWORD="$PG_PASSWORD" \
  -e POSTGRES_DB=famat \
  -v famat-dev-pg:/var/lib/postgresql/data \
  "$PG_IMAGE" >/dev/null

# -------------------------------------------------------------------- Garage

if [ ! -f "$STATE/garage.toml" ]; then
  sed "s/__RPC_SECRET__/$(openssl rand -hex 32)/" "$ROOT/dev/garage.toml.template" > "$STATE/garage.toml"
fi

"$ENGINE" rm -f "$GARAGE_NAME" >/dev/null 2>&1 || true
"$ENGINE" run -d --name "$GARAGE_NAME" \
  -p "127.0.0.1:$S3_PORT:3900" \
  -v "$STATE/garage.toml:/etc/garage.toml:ro" \
  -v famat-dev-garage-meta:/var/lib/garage/meta \
  -v famat-dev-garage-data:/var/lib/garage/data \
  "$GARAGE_IMAGE" >/dev/null

g() { "$ENGINE" exec -e RUST_LOG=warn "$GARAGE_NAME" /garage "$@"; }

for _ in $(seq 1 60); do g status >/dev/null 2>&1 && break; sleep 1; done
g status >/dev/null

NODE_ID="$(g node id -q | cut -d@ -f1)"
if g layout show | grep -q "No nodes currently have a role"; then
  g layout assign -z dc1 -c 1G "$NODE_ID" >/dev/null
  g layout apply --version 1 >/dev/null
fi

g bucket info "$BUCKET" >/dev/null 2>&1 || g bucket create "$BUCKET" >/dev/null
g key info "$KEY_NAME" >/dev/null 2>&1 || g key create "$KEY_NAME" >/dev/null
g bucket allow --read --write --owner "$BUCKET" --key "$KEY_NAME" >/dev/null

KEY_INFO="$(g key info "$KEY_NAME" --show-secret)"
KEY_ID="$(echo "$KEY_INFO" | awk '/Key ID:/ {print $3}')"
SECRET="$(echo "$KEY_INFO" | awk '/Secret key:/ {print $3}')"

# ------------------------------------------------------------- wait + config

for _ in $(seq 1 60); do
  "$ENGINE" exec "$PG_NAME" pg_isready -U famat -d famat >/dev/null 2>&1 && break
  sleep 1
done
"$ENGINE" exec "$PG_NAME" pg_isready -U famat -d famat >/dev/null

umask 077
cat > "$STATE/dev.env" <<EOF
DATABASE_URL=postgres://famat:$PG_PASSWORD@127.0.0.1:$PG_PORT/famat
S3_ENDPOINT=http://127.0.0.1:$S3_PORT
S3_REGION=garage
S3_BUCKET=$BUCKET
S3_ACCESS_KEY_ID=$KEY_ID
S3_SECRET_ACCESS_KEY=$SECRET
EOF

echo "Postgres on 127.0.0.1:$PG_PORT, Garage S3 on 127.0.0.1:$S3_PORT"
echo "Settings in $STATE/dev.env"
