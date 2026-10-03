#!/usr/bin/env bash
# ADR-294 D16: callgrind over the corpus, one unmodified lowering per program.
#
#   CARGO_TARGET_DIR=target/ty-census RUSTFLAGS="-C symbol-mangling-version=v0" \
#     CARGO_PROFILE_RELEASE_DEBUG=1 cargo build --release -p nikaia
#   benches/ty-census/run.sh <out-dir> [--cache-sim | --count]
#
# `--count` runs census.py's build instead (target/ty-count), without
# valgrind, and keeps each run's `ty-census` line in the log: counts only.
#
# v0 mangling keeps generic arguments in symbol names, which is what lets
# attribute.py tell `drop_in_place::<Vec<Ty>>` from `drop_in_place::<Vec<Expr>>`.
# It changes no code the compiler runs.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
out="$1"; shift || true
extra=()
bin="$root/target/ty-census/release/nikaia"
tool=(valgrind --tool=callgrind)
case "${1:-}" in
  --cache-sim) extra=(--cache-sim=yes) ;;
  --count) bin="$root/target/ty-count/release/nikaia"; tool=(env NIKAIA_TY_CENSUS=1) ;;
esac
mkdir -p "$out"
printf 'fn main() {\n}\n' > "$out/empty.nika"
corpus=("$out/empty.nika")
while IFS= read -r f; do corpus+=("$f"); done < <(
  cd "$root" && find examples tests/samples benches crates/nikaia/tests/fixtures \
    -name '*.nika' -not -path '*/target/*' | sort | sed "s|^|$root/|")
for f in "${corpus[@]}"; do
  name="$(echo "${f#$root/}" | tr '/' '_')"
  dir="$(dirname "$f")"
  if (cd "$dir" && "${tool[@]}" ${extra[@]+"${extra[@]}"} $([[ ${tool[0]} == valgrind ]] && echo "--compress-strings=no --compress-pos=no --callgrind-out-file=$out/$name.cg") "$bin" lower "$f" --no-cache \
      --output "$out/$name.rs" >"$out/$name.log" 2>&1); then
    echo "ok   $name $(wc -l < "$f")"
  else
    echo "FAIL $name"
  fi
done
# The eleven tool modules and text.nika, as `nikaia lower-std` checks them.
(cd "$root" && "${tool[@]}" ${extra[@]+"${extra[@]}"} $([[ ${tool[0]} == valgrind ]] && echo "--compress-strings=no --compress-pos=no --callgrind-out-file=$out/lower-std.cg") "$bin" lower-std >"$out/lower-std.log" 2>&1) \
  && echo "ok   lower-std" || echo "FAIL lower-std"
