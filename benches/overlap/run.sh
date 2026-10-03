#!/usr/bin/env bash
# The table in ADR-292 See README.md.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:-$(mktemp -d)}
mkdir -p "$out"

cargo build --manifest-path "$root/benches/overlap/Cargo.toml" --release --quiet
bin="$root/target/release"
cd "$out"

printf '%12s  %11s  %11s  %11s  %8s  %8s\n' \
    'bytes/file' 'strict (ms)' 'scope (ms)' 'join (ms)' 'scope' 'join'
for size in 5 65536 262144 524288 1048576 4194304 8388608; do
    # base64 of urandom, truncated to the exact size: incompressible, and
    # ASCII, so `read_to_string` validates it exactly as the real program
    # would. `truncate` rather than a second `head` - a `head` that closes the
    # pipe early would fail the run under `pipefail`, not shorten the file.
    for name in eins zwei; do
        head -c "$size" /dev/urandom | base64 > "$name.txt"
        truncate -s "$size" "$name.txt"
    done

    # Enough runs that a pair costs more than the clock's resolution, few
    # enough that the large sizes finish.
    if   [ "$size" -lt 100000 ];  then n=20000
    elif [ "$size" -lt 2000000 ]; then n=2000
    else n=200
    fi

    for who in strict scope join; do "$bin/$who" "$n" >/dev/null; done   # warm
    s=$("$bin/strict" "$n" | awk '{print $2}')
    c=$("$bin/scope"  "$n" | awk '{print $2}')
    j=$("$bin/join"   "$n" | awk '{print $2}')
    printf '%12s  %11s  %11s  %11s  %7.2fx  %7.2fx\n' \
        "$size" "$s" "$c" "$j" \
        "$(awk -v a="$s" -v b="$c" 'BEGIN{print a/b}')" \
        "$(awk -v a="$s" -v b="$j" 'BEGIN{print a/b}')"
done
