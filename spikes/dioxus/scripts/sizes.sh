#!/usr/bin/env bash
# Print raw / gzip -9 / brotli -q 11 sizes of every file under a bundle's public/ dir.
# Usage: scripts/sizes.sh <bundle-dir>
find "$1/public" -type f | sort | while read -r f; do
  raw=$(wc -c < "$f")
  gz=$(gzip -9c "$f" | wc -c)
  if command -v brotli >/dev/null; then br=$(brotli -c -q 11 "$f" | wc -c); else br=n/a; fi
  printf "%-50s raw=%10d  gzip9=%9d  brotli=%9s\n" "$(basename "$f")" "$raw" "$gz" "$br"
done
