#!/usr/bin/env bash
# Log in, upload FILE at RATE, and print the server's RSS trace (KiB, every 0.5 s).
# Usage: PASSCODE=... scripts/upload-rss.sh <base-url> <file> [rate]
set -uo pipefail
BASE="$1"; FILE="$2"; RATE="${3:-15M}"
T="$(mktemp -d)"
csrf_of() { grep -o 'name="csrf" value="[^"]*"' "$1" | head -1 | sed 's/.*value="//; s/"$//'; }
curl -s "$BASE/login" > "$T/l.html"
C="$(curl -s -i "$BASE/api/login" --data-urlencode "csrf=$(csrf_of "$T/l.html")" \
  --data-urlencode "passcode=$PASSCODE" | grep -io 'famat_session=[0-9a-f]*')"
curl -s -b "$C" "$BASE/admin" > "$T/a.html"
PID="$(lsof -ti tcp:"${BASE##*:}" -sTCP:LISTEN | head -1)"
echo "RSS before: $(ps -o rss= -p "$PID") KiB"
( while kill -0 "$PID" 2>/dev/null; do ps -o rss= -p "$PID"; sleep 0.5; done ) > "$T/rss.txt" &
S=$!
curl -s -o /dev/null -b "$C" -F "csrf=$(csrf_of "$T/a.html")" -F "file=@$FILE;filename=rss-test.pdf" \
  --limit-rate "$RATE" "$BASE/api/admin/upload" -w 'status=%{http_code} time=%{time_total}s\n'
sleep 1; kill "$S"
echo "RSS trace: $(tr -d ' ' < "$T/rss.txt" | tr '\n' ' ')"
rm -rf "$T"
