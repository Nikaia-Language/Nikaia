#!/usr/bin/env bash
# ADR-038 D4's number: what a pair of operations costs on the runtime that is
# already running, against ADR-292's per-pair thread wake-up.
#
# Five file sizes, and both mechanisms ADR-038 D3 names: `auto` (which is
# completion where the machine has it) and `blocking` pinned, so the fallback
# is measured rather than assumed. See README.md for the method.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:-$(mktemp -d)}
repeats=${2:-7}
mkdir -p "$out"

cargo build --manifest-path "$root/benches/overlap/Cargo.toml" --release --quiet
bin="$root/target/release/runtime"
cd "$out"

echo "machine: $(nproc) cores, $(uname -sr)"
echo "repeats: $repeats per size per mechanism"
# The load matters and is printed rather than assumed: on a box this small a
# neighbour's build moves the large sizes by a factor and leaves the small
# ones - where the vehicle rather than the payload is the answer - alone.
echo "loadavg before: $(cut -d' ' -f1-3 /proc/loadavg)"
echo

for method in auto blocking; do
    # Two I/O workers, so the fallback has something to overlap a pair *with*.
    # One worker serialises a pair, which is the honest answer for a machine
    # that has neither a completion queue nor a second core to spare.
    printf 'io-method = "%s"\nio-workers = 2\n' "$method" > nikaia-runtime.toml
    export NIKAIA_RUNTIME_CONFIG="$out/nikaia-runtime.toml"

    for size in 5 4096 65536 262144 1048576; do
        for name in eins zwei; do
            head -c "$size" /dev/urandom | base64 > "$name.txt"
            truncate -s "$size" "$name.txt"
        done
        if   [ "$size" -lt 100000 ];  then n=20000
        elif [ "$size" -lt 500000 ]; then n=4000
        else n=1000
        fi
        echo "=== io-method=$method, $size bytes/file, $n pairs per repeat"
        "$bin" "$n" "$repeats"
        echo "loadavg after: $(cut -d' ' -f1-3 /proc/loadavg)"
        echo
    done
done
