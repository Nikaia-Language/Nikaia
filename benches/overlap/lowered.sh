#!/usr/bin/env bash
# ADR-292 D6's number, end to end: what the *lowering* costs, not what the
# `std` function costs.
#
# `runtime.sh` measures `nikaia_std` from Rust. This measures a `.nika` program
# through the whole pipeline - the analysis, the emitter, `cargo`, the runtime -
# at `user_parallelism = no`, against the same program lowered with
# `--ordering strict`. Two binaries, one source, and the only difference between
# them is the pair.
#
# Both of ADR-303 D3's mechanisms are measured, because D10's decision is about
# the second one: on the completion path a pair is free, and on the fallback the
# pair is performed in written order rather than for ~38 µs a pair. The
# fallback's column is therefore the check that the decision is in the code.
#
# See README.md for the method and docs/history/runtime-cost.md §6 for the numbers.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:-$(mktemp -d)}
pairs=${2:-200000}
repeats=${3:-9}
size=${4:-5}

mkdir -p "$out"
cargo build --manifest-path "$root/Cargo.toml" -p nikaia --release --quiet
nikaia="$root/target/release/nikaia"

echo "machine: $(nproc) cores, $(uname -sr)"
echo "program: benches/overlap/pair.nika, $pairs pairs per run, $repeats repeats"
echo "loadavg before: $(cut -d' ' -f1-3 /proc/loadavg)"
echo

# One `cargo` target directory for both projects, so the runtime is built once
# and neither binary pays for the other's dependencies.
export CARGO_TARGET_DIR="$out/cargo"

for ordering in effects strict; do
    project="$out/$ordering"
    mkdir -p "$project/src"
    cp "$root/benches/overlap/pair.nika" "$project/src/main.nika"
    cat > "$project/nikaia.toml" <<TOML
[package]
name = "pair_$ordering"
version = "0.1.0"

[build]
user-parallelism = "no"
ordering = "$ordering"

[build.x86_64-linux]
opt-level = 3
TOML
    (cd "$project" && "$nikaia" build >/dev/null)

    # What was built, asserted rather than assumed: a measurement of two
    # identical binaries would be a very stable nothing.
    if grep -q 'task::read_pair' "$project/target/nikaia/gen/pair_$ordering.rs"; then
        echo "$ordering: the pair is lowered onto task::read_pair"
    else
        echo "$ordering: no completion pair in the emitted Rust"
    fi
done
echo

for method in auto blocking; do
    printf 'io-method = "%s"\nio-workers = 2\n' "$method" > "$out/nikaia-runtime.toml"
    export NIKAIA_RUNTIME_CONFIG="$out/nikaia-runtime.toml"

    for ordering in effects strict; do
        at="$out/$ordering/run"
        mkdir -p "$at"
        for name in eins zwei; do
            head -c "$size" /dev/urandom | base64 > "$at/$name.txt"
            truncate -s "$size" "$at/$name.txt"
        done
    done

    echo "=== io-method=$method, $size bytes/file"
    # Warm-up, so no repeat below includes a cold page cache or the first
    # `io_uring_enter`.
    for ordering in effects strict; do
        (cd "$out/$ordering/run" && "$CARGO_TARGET_DIR/debug/pair_$ordering" 64 >/dev/null)
    done

    for repeat in $(seq 1 "$repeats"); do
        line="repeat $repeat:"
        for ordering in effects strict; do
            began=$(date +%s%N)
            (cd "$out/$ordering/run" && "$CARGO_TARGET_DIR/debug/pair_$ordering" "$pairs" >/dev/null)
            ended=$(date +%s%N)
            line="$line $ordering $(( (ended - began) / 1000 ))µs"
        done
        echo "  $line"
    done
    echo "loadavg after: $(cut -d' ' -f1-3 /proc/loadavg)"
    echo
done
