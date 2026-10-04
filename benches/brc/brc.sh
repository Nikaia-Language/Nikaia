#!/usr/bin/env bash
# What `examples/1brc.nika` costs against the same aggregation written by hand.
#
# Three programs, one input, and the output compared byte for byte first — a
# difference in speed is only worth reading once nothing differs in what was
# computed.
#
# The other benches here have the binary time itself, because they measure one
# operation. This one cannot: the third program is produced by the compiler and
# there is nothing to instrument inside it. So all three are timed the same
# way, as whole processes — which is also the unit that matters for a program
# that reads a file and prints a line.
#
# Absolutes do not travel: `docs/history/runtime-cost.md` §6.3 has this box
# moving by 1.4–1.9× from one day to the next. The script prints the machine
# and the load with the table; the README quotes ratios.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
rows=${1:-8000000}
repeats=${2:-5}
work=${TMPDIR:-/tmp}/brc-bench
input="$work/measurements.txt"

mkdir -p "$work"

cargo build --manifest-path "$root/Cargo.toml" --release --quiet -p brc-bench -p nikaia
hand="$root/target/release/handwritten"
nikaia="$root/target/release/nikaia"

# The Nikaia half is built the way a user builds it: a project with a
# `nikaia.toml`, per `examples/1brc.nika`'s own header. It is built twice, at
# both `user-parallelism` settings, because the hand-written halves are
# single-threaded and a four-core number against a one-core number is not a
# comparison. `no` is the row that answers "is the grammar worth writing"; `yes`
# is what ADR-296's frame-plus-monoid adds on top of the answer.
proj="$work/project"
mkdir -p "$proj/src"
cp "$root/examples/1brc.nika" "$proj/src/main.nika"

# The fairest build Nikaia has against `cargo build --release`: its own
# optimizations on (they change nothing in this program's Rust, and say so by
# producing the same binary), and `incremental` off, which is the one setting of
# the generated profile that trades run time for rebuild time (ADR-002 D5).
nikaia_toml() {
cat > "$proj/nikaia.toml" <<TOML
[package]
name = "brc"
version = "0.1.0"

[build]
user-parallelism = "$1"
optimization = "remove-bounds-checks:aggressive,remove-overflow-checks:aggressive"

[build.x86_64-linux]
opt-level = 3
incremental = false
TOML
}

if [ ! -s "$input" ] || [ "$(wc -l < "$input")" -ne "$rows" ]; then
    "$hand" gen "$rows" "$input"
fi

nikaia_toml no
( cd "$proj" && "$nikaia" build >/dev/null 2>&1 )

echo "machine: $(nproc) cores, $(uname -sr)"
echo "rustc:   $(rustc --version)"
echo "input:   $rows rows, $(du -h "$input" | cut -f1), 413 stations"
echo "loadavg before: $(cut -d' ' -f1-3 /proc/loadavg)"
echo

# --- the same answer, first ------------------------------------------------

run_nikaia() { ( cd "$proj" && "$nikaia" run -- "$input" 2>/dev/null | tail -1 ); }

"$hand" naive "$input" > "$work/out.naive"
"$hand" tuned "$input" > "$work/out.tuned"
run_nikaia                > "$work/out.nikaia"

cmp -s "$work/out.naive" "$work/out.tuned" \
    || { echo "FAIL: naive and tuned disagree"; exit 1; }
cmp -s "$work/out.naive" "$work/out.nikaia" \
    || { echo "FAIL: Nikaia and hand-written disagree"; exit 1; }
nikaia_toml yes
( cd "$proj" && "$nikaia" build >/dev/null 2>&1 )
run_nikaia > "$work/out.nikaia.par"
cmp -s "$work/out.naive" "$work/out.nikaia.par" \
    || { echo "FAIL: the parallel parse disagrees - ADR-296 D10's whole claim"; exit 1; }
nikaia_toml no
( cd "$proj" && "$nikaia" build >/dev/null 2>&1 )

printf 'all four runs agree, byte for byte (%s stations)\n\n' \
    "$(tr ',' '\n' < "$work/out.naive" | wc -l)"

# --- then the clock --------------------------------------------------------

# Best of N. The best run rather than the mean, because on a shared box the
# distribution's tail is other people's work and the minimum is the closest
# thing to the program's own cost.
best() {
    local label=$1 b=999999 t
    shift
    for _ in $(seq "$repeats"); do
        t=$( { time -p "$@" >/dev/null 2>&1; } 2>&1 | awk '/^real/ { print $2 }' )
        b=$(awk -v a="$b" -v c="$t" 'BEGIN { print (c < a) ? c : a }')
    done
    printf '%-26s %7s s\n' "$label" "$b"
}

# `nikaia run` re-checks the project before running it. The check is cached and
# costs well under a tenth of a second here; it is inside the Nikaia numbers
# and said so rather than subtracted, there being no supported way to invoke
# the generated binary without it.
nikaia_cmd=(bash -c 'cd "$1" && "$2" run -- "$3"' _ "$proj" "$nikaia" "$input")

best "Rust, naive (1 core)" "$hand" naive "$input"
best "Nikaia, no (1 core)"  "${nikaia_cmd[@]}"
best "Rust, tuned (1 core)" "$hand" tuned "$input"

nikaia_toml yes
( cd "$proj" && "$nikaia" build >/dev/null 2>&1 )
best "Nikaia, yes ($(nproc) cores)" "${nikaia_cmd[@]}"

echo
echo "loadavg after:  $(cut -d' ' -f1-3 /proc/loadavg)"
echo "best of $repeats; only the ratios travel between machines."
