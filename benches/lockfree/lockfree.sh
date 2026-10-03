#!/usr/bin/env bash
# What a compare-and-swap loop costs against the two lock shapes this compiler
# writes — the number ADR-281's open door turns on.
#
# The machine and the load are printed with the table, because absolutes on a
# box of this class move 1.4-1.9x from one day to the next and only the ratios
# travel (`docs/history/runtime-cost.md` §6.3).
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
n=${1:-5000000}
repeats=${2:-9}
threads=${3:-$(nproc)}

cargo build --manifest-path "$root/benches/lockfree/Cargo.toml" --release --quiet

echo "machine: $(nproc) cores, $(uname -sr)"
echo "rustc:   $(rustc --version)"
echo "loadavg before: $(cut -d' ' -f1-3 /proc/loadavg)"
echo
"$root/target/release/lockfree" "$n" "$repeats" "$threads"
echo "loadavg after:  $(cut -d' ' -f1-3 /proc/loadavg)"
