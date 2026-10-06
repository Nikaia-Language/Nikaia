#!/usr/bin/env bash
# What fallible collections cost against today's `Vec`, `String` and
# `HashMap` (ADR-327 D4, D5; #490 step 1): instructions and mispredicted
# branches per row and per task, counted with callgrind. It prints the table
# `README.md` keeps.
#
#   benches/fallible/fallible.sh                 200 000 rows, 100 000 tasks
#   benches/fallible/fallible.sh 50000 20000     quicker
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
rows=${1:-200000}
tasks=${2:-100000}
work=${TMPDIR:-/tmp}/fallible-bench
mkdir -p "$work"
command -v valgrind >/dev/null || { echo "fallible.sh needs valgrind (callgrind)"; exit 1; }

cargo build --manifest-path "$root/Cargo.toml" --release --quiet -p fallible-bench -p brc-bench
bin="$root/target/release"
input="$work/measurements-$rows.txt"
[ -s "$input" ] || "$bin/gen" "$rows" "$input" 2>/dev/null
empty="$work/empty.txt"
: > "$empty"

# The same output first, byte for byte, or the counts mean nothing.
for w in brc tasks; do
    arg=$input; [ "$w" = tasks ] && arg=$tasks
    "$bin/${w}_std" "$arg" > "$work/$w.std"
    "$bin/${w}_fallible" "$arg" > "$work/$w.fallible"
    cmp -s "$work/$w.std" "$work/$w.fallible" || { echo "$w: the two print something else"; exit 1; }
done

# Ir and mispredicted branches (Bcm + Bim) of one run.
count() {
    valgrind --tool=callgrind --branch-sim=yes --callgrind-out-file="$work/cg.$1.$3" \
        "$bin/$1" "$2" > /dev/null 2>&1
    awk '/^summary/ { print $2, $4 + $6 }' "$work/cg.$1.$3"
}

# A run over nothing is subtracted, so what is left is the rows' or the
# tasks' own.
row() {
    local name=$1 work_arg=$2 none_arg=$3 n=$4
    read -r ir0 m0 < <(count "$name" "$none_arg" none)
    read -r ir m < <(count "$name" "$work_arg" all)
    awk -v p="$name" -v n="$n" -v ir="$ir" -v ir0="$ir0" -v m="$m" -v m0="$m0" \
        'BEGIN { printf "| `%s` | %.1f | %.2f |\n", p, (ir - ir0) / n, (m - m0) / n }'
}

echo "| program | instructions | mispredicted |"
echo "|---|---:|---:|"
row brc_std "$input" "$empty" "$rows"
row brc_fallible "$input" "$empty" "$rows"
row tasks_std "$tasks" 0 "$tasks"
row tasks_fallible "$tasks" 0 "$tasks"
