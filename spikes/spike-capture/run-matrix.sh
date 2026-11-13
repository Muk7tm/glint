#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Phase 0 measurement matrix for spike-capture (read-only on the current DISPLAY).
#
#   run-matrix.sh <label> <results.tsv> [none|xrender|glx]
#
# With a compositor argument, starts `picom --config /dev/null` with that backend
# for the duration of the runs (bounded by `timeout`) and stops it afterwards.
set -euo pipefail

label=$1
out=$2
compositor=${3:-none}
bin="$(dirname "$0")/../target/release/spike-capture"
count=${COUNT:-1000000}
secs=${SECS:-20}

picom_pid=
cleanup() {
    if [[ -n $picom_pid ]]; then
        kill "$picom_pid" 2>/dev/null || true
        wait "$picom_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT

case $compositor in
none) ;;
xrender) timeout 900 picom --config /dev/null --backend xrender >/dev/null 2>&1 & picom_pid=$! ;;
glx) timeout 900 picom --config /dev/null --backend glx --vsync >/dev/null 2>&1 & picom_pid=$! ;;
*) echo "unknown compositor $compositor" >&2; exit 2 ;;
esac
sleep 3
"$bin" info | grep -E '^compositor'

for target in root window; do
    for method in x11rb raw shm; do
        "$bin" bench --target "$target" --method "$method" --count "$count" --label "$label" | tee /dev/stderr | grep '^RESULT' >>"$out"
    done
done
for rate in 0 500 2000 max; do
    "$bin" tax --rate "$rate" --secs "$secs" --method raw --label "$label" | tee /dev/stderr | grep '^RESULT' >>"$out"
done
"$bin" stalls --secs 10 --label "$label" >&2
