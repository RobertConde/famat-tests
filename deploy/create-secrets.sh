#!/usr/bin/env bash
# Create the Podman secrets the FAMAT containers use. Generates the random
# ones and asks for the Cloudflare Tunnel token. A secret that already exists
# is kept, so this is safe to re-run.
#
#   sudo deploy/create-secrets.sh
#   sudo deploy/create-secrets.sh < token.txt   # the token from stdin
#
# The values live only in Podman's secret store on this server
# (/var/lib/containers/storage/secrets). Never write them into git.
#
#   famat-db-password            Postgres password (app and Postgres)
#   famat-s3-access-key-id       Garage key ID, "GK" + 24 hex digits
#   famat-s3-secret-access-key   Garage secret key, 64 hex digits
#   famat-garage-rpc-secret      Garage RPC secret, 64 hex digits
#   famat-tunnel-token           cloudflared tunnel token (from Cloudflare)
#
# The Admin passcode is not a secret here: `famat set-passcode` stores its
# argon2 hash in Postgres (docs/deploy/admin.md).
set -euo pipefail

create() {
  local name="$1" value="$2"
  if podman secret exists "$name"; then
    echo "$name: exists, kept."
  else
    printf '%s' "$value" | podman secret create "$name" - >/dev/null
    echo "$name: created."
  fi
}

create famat-db-password "$(openssl rand -hex 24)"
create famat-s3-access-key-id "GK$(openssl rand -hex 12)"
create famat-s3-secret-access-key "$(openssl rand -hex 32)"
create famat-garage-rpc-secret "$(openssl rand -hex 32)"

if podman secret exists famat-tunnel-token; then
  echo "famat-tunnel-token: exists, kept."
else
  if [ -t 0 ]; then
    read -r -s -p "Cloudflare Tunnel token (from the tunnel's install command): " token
    echo
  else
    read -r token
  fi
  if [ -z "$token" ]; then
    echo "No token given; famat-tunnel-token not created." >&2
    exit 1
  fi
  create famat-tunnel-token "$token"
fi
