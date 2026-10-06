#!/usr/bin/env bash
# What a memory block per task costs, against the system allocator
# (ADR-327 D4-D8, #490 step 2): instructions and mispredicted branches per
# task, counted with callgrind. It prints the table `README.md` keeps.
#
#   benches/taskblock/taskblock.sh           100 000 tasks
#   benches/taskblock/taskblock.sh 20000     quicker
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
tasks=${1:-100000}
work=${TMPDIR:-/tmp}/taskblock-bench
mkdir -p "$work"
command -v valgrind >/dev/null || { echo "taskblock.sh needs valgrind (callgrind)"; exit 1; }

cargo build --manifest-path "$root/Cargo.toml" --release --quiet -p taskblock-bench
bin="$root/target/release"
programs="global budget block"

# The same output first, byte for byte, or the counts mean nothing.
"$bin/global" "$tasks" > "$work/global.out"
for p in $programs; do
    "$bin/$p" "$tasks" > "$work/$p.out"
    cmp -s "$work/global.out" "$work/$p.out" || { echo "$p prints something else"; exit 1; }
done

# Ir, Bcm and Bim of a run; a run of no task is subtracted, so what is left is
# the tasks' own.
count() {
    valgrind --tool=callgrind --branch-sim=yes --callgrind-out-file="$work/cg.$1.$2" \
        "$bin/$1" "$2" > /dev/null 2>&1
    awk '/^summary/ { print $2, $4 + $6 }' "$work/cg.$1.$2"
}

echo "| allocator | instructions / task | mispredicted / task |"
echo "|---|---:|---:|"
for p in $programs; do
    read -r ir0 mis0 < <(count "$p" 0)
    read -r ir mis < <(count "$p" "$tasks")
    awk -v p="$p" -v t="$tasks" -v ir="$ir" -v ir0="$ir0" -v m="$mis" -v m0="$mis0" \
        'BEGIN { printf "| `%s` | %.0f | %.1f |\n", p, (ir - ir0) / t, (m - m0) / t }'
done
