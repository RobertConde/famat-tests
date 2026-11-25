#!/usr/bin/env bash
# End-to-end curl checks for the four spike items. No JavaScript involved.
# Needs: the server on $BASE, Garage up with sample-a.txt, sample-b.txt,
# big-a.bin and big-b.bin uploaded (see FINDINGS.md), and PASSCODE.
set -uo pipefail
BASE="${BASE:-http://127.0.0.1:8080}"
PASSCODE="${PASSCODE:?set PASSCODE}"
HERE="$(cd "$(dirname "$0")" && pwd)"
T="$(mktemp -d)"
text() { python3 -I "$HERE/textof.py" "$@"; }
hdrs() { grep -iE '^(HTTP|location|set-cookie|content-type|content-disposition|transfer-encoding|content-length)' | sed -E 's/famat_session=[0-9a-f]{64}/famat_session=<64 hex>/; s/\r$//'; }
csrf_of() { grep -o 'name="csrf" value="[^"]*"' "$1" | head -1 | sed 's/.*value="//; s/"$//'; }

echo "== 1. Search, server-rendered (curl, no JS)"
curl -s "$BASE/?year=2024&level=Algebra%20I" > "$T/search.html"
echo "HTML bytes: $(wc -c < "$T/search.html")"
text js-state count results < "$T/search.html"
curl -s "$BASE/?year=&level=Theta" | text count results
curl -s "$BASE/" | text count
echo "hydration script tags:"; grep -oE '<script[^>]*src="[^"]*"' "$T/search.html" | sed 's/^/  /'
grep -oE '<link[^>]*rel="preload"[^>]*>' "$T/search.html" | sed 's/^/  /'

echo; echo "== 2. Admin passcode login"
echo "-- anonymous GET /admin"; curl -s -i "$BASE/admin" | hdrs
echo "-- anonymous GET /api/admin/info"; curl -s -i "$BASE/api/admin/info" | { hdrs; tail -1; echo; }
curl -s "$BASE/login" > "$T/login.html"
CSRF="$(csrf_of "$T/login.html")"
echo "-- login form CSRF token: ${CSRF:0:20}..."
echo "-- POST wrong passcode"
curl -s -i "$BASE/api/login" --data-urlencode "csrf=$CSRF" --data-urlencode "passcode=wrong" | hdrs
echo "-- POST forged CSRF token"
curl -s -i "$BASE/api/login" --data-urlencode "csrf=1.deadbeef" --data-urlencode "passcode=$PASSCODE" | hdrs
echo "-- POST valid token, cross-site Origin"
curl -s -i "$BASE/api/login" -H "Origin: https://evil.example" --data-urlencode "csrf=$CSRF" --data-urlencode "passcode=$PASSCODE" | hdrs
echo "-- POST valid token + passcode"
curl -s -i "$BASE/api/login" -H "Origin: $BASE" --data-urlencode "csrf=$CSRF" --data-urlencode "passcode=$PASSCODE" > "$T/login-ok.txt"
hdrs < "$T/login-ok.txt"
SID="$(grep -io 'famat_session=[0-9a-f]*' "$T/login-ok.txt" | cut -d= -f2)"
C="famat_session=$SID"
echo "-- GET /admin with cookie"
curl -s -i -b "$C" "$BASE/admin" > "$T/admin.html"; head -1 "$T/admin.html"
text js-state objects < "$T/admin.html"
ACSRF="$(csrf_of "$T/admin.html")"

echo; echo "== 3. Upload into Garage (multipart form POST)"
printf 'small spike upload %s\n' "$(date)" > "$T/small note.txt"
echo "-- no CSRF field"
curl -s -i -b "$C" -F "file=@$T/small note.txt" "$BASE/api/admin/upload" | { hdrs; tail -1; echo; }
echo "-- anonymous"
curl -s -i -F "csrf=$ACSRF" -F "file=@$T/small note.txt" "$BASE/api/admin/upload" | hdrs
echo "-- small file"
curl -s -i -b "$C" -F "csrf=$ACSRF" -F "file=@$T/small note.txt" "$BASE/api/admin/upload" | hdrs
head -c 20000000 /dev/urandom > "$T/twenty-mb.pdf"
echo "-- 20 MB file (3 multipart parts)"
curl -s -i -b "$C" -F "csrf=$ACSRF" -F "file=@$T/twenty-mb.pdf;type=application/pdf" "$BASE/api/admin/upload" -w 'time_total=%{time_total}s\n' | { hdrs; grep time_total; }
echo "-- Admin page lists:"
curl -s -b "$C" "$BASE/admin?uploaded=uploads/twenty-mb.pdf" | text uploaded objects
echo "-- round trip check of the 20 MB object"
"$HERE/../garage/s3curl.sh" -o "$T/back.pdf" /uploads/twenty-mb.pdf
cmp "$T/twenty-mb.pdf" "$T/back.pdf" && echo "  identical ($(wc -c < "$T/back.pdf") bytes)"

echo; echo "== 4. Streaming zip (plain axum route)"
curl -s -D "$T/zh.txt" -o "$T/two.zip" "$BASE/download/zip?keys=sample-a.txt,sample-b.txt"
hdrs < "$T/zh.txt"
unzip -l "$T/two.zip" | sed 's/^/  /'
unzip -p "$T/two.zip" sample-a.txt | sed 's/^/  sample-a.txt: /'
echo "-- missing key"; curl -s -i "$BASE/download/zip?keys=nope.pdf,sample-a.txt" | sed -n '1p'
echo "-- 2 x 150 MB objects; server RSS sampled every 0.2 s"
PID="$(lsof -ti tcp:"${BASE##*:}" -sTCP:LISTEN | head -1)"
echo "  RSS before: $(ps -o rss= -p "$PID") KiB"
( while kill -0 "$PID" 2>/dev/null; do ps -o rss= -p "$PID"; sleep 0.2; done ) > "$T/rss.txt" &
SAMPLER=$!
curl -s -o "$T/big.zip" "$BASE/download/zip?keys=big-a.bin,big-b.bin" \
  -w '  size=%{size_download} bytes  time=%{time_total}s  ttfb=%{time_starttransfer}s\n'
kill "$SAMPLER"
echo "  RSS peak during download: $(sort -n "$T/rss.txt" | tail -1) KiB"
unzip -l "$T/big.zip" | sed 's/^/  /'
unzip -t "$T/big.zip" | tail -1 | sed 's/^/  /'
echo "-- same zip to a slow client (curl --limit-rate 30M): backpressure, not buffering"
( while kill -0 "$PID" 2>/dev/null; do ps -o rss= -p "$PID"; sleep 0.2; done ) > "$T/rss2.txt" &
SAMPLER=$!
curl -s -o "$T/big2.zip" --limit-rate 30M "$BASE/download/zip?keys=big-a.bin,big-b.bin" \
  -w '  size=%{size_download} bytes  time=%{time_total}s\n'
kill "$SAMPLER"
echo "  RSS samples: $(wc -l < "$T/rss2.txt" | tr -d ' '), peak $(sort -n "$T/rss2.txt" | tail -1) KiB"
cmp "$T/big.zip" "$T/big2.zip" && echo "  identical to the fast download"

echo; echo "== Logout"
curl -s -i -b "$C" "$BASE/api/logout" --data-urlencode "csrf=$ACSRF" | hdrs
echo "-- old cookie after logout:"; curl -s -i -b "$C" "$BASE/admin" | hdrs
rm -rf "$T"
