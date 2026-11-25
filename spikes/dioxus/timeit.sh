#!/usr/bin/env bash
# Usage: timeit.sh <log> <cmd...>   Runs cmd, writes its output and wall time to <log>.
log="$1"; shift
start=$(date +%s)
"$@" > "$log" 2>&1
code=$?
end=$(date +%s)
echo "EXIT $code WALL $((end - start))s: $*" | tee -a "$log"
exit $code
