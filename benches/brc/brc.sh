#!/usr/bin/env bash
# What `examples/1brc.nika` costs against the same aggregation written in Rust:
# instructions and mispredicted branches per row, counted with callgrind, and
# each program's length in tokens. It prints the table `README.md` keeps.
#
# Counts, not a clock: on a shared machine the same binaries swap places from
# one round to the next, and a count does not.
#
#   benches/brc/brc.sh            1 000 000 rows
#   benches/brc/brc.sh 200000     quicker, the same table at a smaller size
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
rows=${1:-1000000}
work=${TMPDIR:-/tmp}/brc-bench
input="$work/measurements-$rows.txt"
cache=${XDG_CACHE_HOME:-$HOME/.cache}/nikaia
mkdir -p "$work"

command -v valgrind >/dev/null || { echo "brc.sh needs valgrind (callgrind)"; exit 1; }

# Release builds on both sides: `cargo build --release` for the Rust programs,
# and the compiler itself.
cargo build --manifest-path "$root/Cargo.toml" --release --quiet -p brc-bench -p nikaia
bin="$root/target/release"
nikaia="$bin/nikaia"

[ -s "$input" ] || "$bin/gen" "$rows" "$input" 2>/dev/null

# The Nikaia program is built the way a user builds it: a project with a
# `nikaia.toml`. `opt-level = 3` as Cargo's release profile has it, no LTO on
# either side, the compiler's own optimizations on.
proj="$work/project"
mkdir -p "$proj/src"
cp "$root/examples/1brc.nika" "$proj/src/main.nika"
nikaia_toml() {
cat > "$proj/nikaia.toml" <<TOML
[package]
name = "brc"
version = "0.1.0"

[build]
user-parallelism = "$1"
optimization = "remove-bounds-checks:on,remove-overflow-checks:on"

[build.x86_64-linux]
opt-level = 3
incremental = false
TOML
}

# Builds the project and copies the binary out of the shared build cache, where
# the next build would overwrite it.
build_nikaia() {
    local name=$1 parallel=$2
    nikaia_toml "$parallel"
    ( cd "$proj" && "$nikaia" build >/dev/null 2>&1 ) || { echo "nikaia build failed ($name)"; exit 1; }
    local built
    built=$(find "$cache" -type f -name brc -perm -u+x -printf '%T@ %p\n' | sort -n | tail -1 | cut -d' ' -f2-)
    cp "$built" "$work/nikaia.$name"
}

build_nikaia parallel yes
build_nikaia checked no
# The same program without the overflow checks the language keeps, to say what
# they cost. `RUSTFLAGS` comes after the profile's flags, so it wins.
RUSTFLAGS="-C overflow-checks=off" build_nikaia unchecked no
build_nikaia checked no >/dev/null

# --- the same answer, first ------------------------------------------------

"$bin/naive" "$input" > "$work/out.naive"
for p in "$bin/tuned" "$work/nikaia.checked" "$work/nikaia.unchecked" "$work/nikaia.parallel"; do
    "$p" "$input" | cmp -s - "$work/out.naive" || { echo "FAIL: $(basename "$p") disagrees with naive"; exit 1; }
done

# --- then the counts -------------------------------------------------------

count() {
    local label=$1 tokens=$2
    shift 2
    valgrind --tool=callgrind --branch-sim=yes --callgrind-out-file="$work/cg.out" "$@" >/dev/null 2>&1
    awk -v r="$rows" -v l="$label" -v t="$tokens" '/^summary:/ {
        printf "| %s | %.1f | %.2f | %s |\n", l, $2 / r, ($4 + $6) / r, t }' "$work/cg.out"
}
tokens() { python3 "$root/scripts/tokens.py" "$1" | awk '{print $1}'; }

version=$(sed -n 's/^\*\*Version:\*\* \([0-9.]*\).*/\1/p' "$root/docs/specification/10-nikaia-light.md")
echo "Nikaia $version, $(rustc --version | cut -d' ' -f1-2), $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | sed 's/^ //')"
echo "callgrind, $rows rows, per row; every program prints the same, byte for byte"
echo
echo "| | instructions | mispredictions | tokens |"
echo "|---|---:|---:|---:|"
count 'Rust, `naive`' "$(tokens "$root/benches/brc/src/bin/naive.rs")" "$bin/naive" "$input"
count 'Rust, `tuned`' "$(tokens "$root/benches/brc/src/bin/tuned.rs")" "$bin/tuned" "$input"
count '**Nikaia**' "$(tokens "$root/examples/1brc.nika")" "$work/nikaia.checked" "$input"
count 'Nikaia, overflow checks off' '' "$work/nikaia.unchecked" "$input"
