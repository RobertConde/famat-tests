#!/usr/bin/env bash
# curl with SigV4 against the spike's Garage. Usage: s3curl.sh [curl args] /key
#   s3curl.sh -T file.pdf /a.pdf     # PUT
#   s3curl.sh -o out.pdf /a.pdf      # GET
set -euo pipefail
STATE="${STATE:-$HOME/.cache/famat-garage-spike}"
set -a; . "$STATE/s3.env"; set +a
path="${*: -1}"
args=("${@:1:$#-1}")
exec curl -sS --aws-sigv4 "aws:amz:$S3_REGION:s3" \
  --user "$S3_ACCESS_KEY_ID:$S3_SECRET_ACCESS_KEY" \
  ${args[@]+"${args[@]}"} "$S3_ENDPOINT/$S3_BUCKET$path"
