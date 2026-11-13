#!/usr/bin/env bash
# Compare render modes / terminals on the same workload.
# usage: scripts/bench.sh <label> <command...>   (FLOOD must be set)
set -u
label=$1; shift
out=$(mktemp); rm -f "$out"
export PERF_OUT=$out PERF_EXIT=1 GOATTY_SHELL=$(realpath "$(dirname "$0")")/perf.sh GOATTY_STATS=1 RUST_LOG=warn,rio_vt=error
"$@" > "$out.log" 2>&1 &
pid=$!
for _ in $(seq 1 120); do sleep 0.5; [ -s "$out" ] && break; done
sleep 0.3; kill $pid 2>/dev/null; wait $pid 2>/dev/null
printf '%-14s %s\n' "$label" "$(cat "$out" 2>/dev/null || echo TIMEOUT)"
grep '\[stats\]' "$out.log" | sed 's/^/               /' | head -8
