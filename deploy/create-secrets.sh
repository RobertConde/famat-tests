#!/usr/bin/env bash
# Create the secrets the FAMAT containers use: root-only env files in
# /etc/famat/secrets/, which deploy/compose.yaml reads (env_file). Generates
# the random ones and asks for the Cloudflare Tunnel token. A file that
# already exists is kept, so this is safe to re-run.
#
#   sudo deploy/create-secrets.sh
#   sudo deploy/create-secrets.sh < token.txt   # the token from stdin
#
# The values live only in these files on this server (directory 0700, files
# 0600, owned by root). Never write them into git.
#
#   db.env      POSTGRES_PASSWORD (Postgres, first start only) and
#               PGPASSWORD (the app and the CLI): the same password
#   s3.env      the app's Garage key: S3_ACCESS_KEY_ID ("GK" + 24 hex
#               digits) and S3_SECRET_ACCESS_KEY (64 hex digits), and the
#               same pair as RCLONE_S3_* for the backups' rclone
#   garage.env  GARAGE_RPC_SECRET (64 hex digits)
#   tunnel.env  TUNNEL_TOKEN, this site's own tunnel (from Cloudflare)
#
# The Admin passcode is not a secret here: `famat set-passcode` stores its
# argon2 hash in Postgres (docs/deploy/admin.md).
set -euo pipefail

SECRETS="${FAMAT_SECRETS:-/etc/famat/secrets}"
umask 077
install -d -m 0700 "$SECRETS"

# write NAME CONTENT: create $SECRETS/NAME unless it exists.
write() {
  if [ -e "$SECRETS/$1" ]; then
    echo "$SECRETS/$1: exists, kept."
  else
    printf '%s\n' "$2" > "$SECRETS/$1.tmp"
    mv "$SECRETS/$1.tmp" "$SECRETS/$1"
    echo "$SECRETS/$1: created."
  fi
}

db_password="$(openssl rand -hex 24)"
write db.env "POSTGRES_PASSWORD=$db_password
PGPASSWORD=$db_password"

key_id="GK$(openssl rand -hex 12)"
secret_key="$(openssl rand -hex 32)"
write s3.env "S3_ACCESS_KEY_ID=$key_id
S3_SECRET_ACCESS_KEY=$secret_key
RCLONE_S3_ACCESS_KEY_ID=$key_id
RCLONE_S3_SECRET_ACCESS_KEY=$secret_key"

write garage.env "GARAGE_RPC_SECRET=$(openssl rand -hex 32)"

if [ -e "$SECRETS/tunnel.env" ]; then
  echo "$SECRETS/tunnel.env: exists, kept."
else
  if [ -t 0 ]; then
    read -r -s -p "Cloudflare Tunnel token (from the famat tunnel's install command): " token
    echo
  else
    read -r token
  fi
  # A token is base64 (an "eyJ…" string). Anything else would break the env
  # file, or is a pasted command rather than the token alone.
  if ! [[ "$token" =~ ^[A-Za-z0-9+/=_-]+$ ]]; then
    echo "That is not a tunnel token (expected the long eyJ… string alone); tunnel.env not created." >&2
    exit 1
  fi
  write tunnel.env "TUNNEL_TOKEN=$token"
fi
