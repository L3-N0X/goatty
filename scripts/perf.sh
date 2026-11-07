#!/usr/bin/env bash
# Throughput workload, run *inside* a terminal. Appends timings to $PERF_OUT.
FLOOD=${FLOOD:?set FLOOD to a large file}
sleep 1
s=$(date +%s.%N); seq 1 3000000; e=$(date +%s.%N); T1=$(echo "$e - $s" | bc)
s=$(date +%s.%N); for i in 1 2 3; do cat "$FLOOD"; done; e=$(date +%s.%N); T2=$(echo "$e - $s" | bc)
MB=$(( $(stat -c %s "$FLOOD") * 3 / 1000000 ))
echo "seq(23MB): ${T1}s  colored(${MB}MB): ${T2}s  -> $(echo "scale=1; $MB / $T2" | bc) MB/s" >> "${PERF_OUT:-/dev/null}"
[ -n "$PERF_EXIT" ] && exit 0
exec zsh -l
