#!/bin/sh
# Which of CI's slower jobs a change needs, written to $GITHUB_OUTPUT.
#
# * `unsafe` - the crates under crates/unsafe/ are their own workspaces and
#   nothing outside them is in their build, so their tests, clippy and Miri say
#   something new only when one of them, or the script that checks them,
#   changed.
# * `floor` - the whole suite on the oldest Rust Nikaia claims (ADR-305). Any
#   change can reach for an API newer than the floor, so it runs on every push
#   to develop or main and on the weekly schedule; on a pull request only where
#   the change is to a manifest, the lock file, the toolchain or the floor
#   itself - the places a floor is declared.
# * `arm64` - the whole suite on a 64-bit ARM machine. Nothing in the tree
#   names that architecture, so what it can break is what differs between
#   machines underneath: the runtime and its `unsafe` crates (atomics, memory
#   ordering, io_uring, mmap), and the dependencies. It runs on every push to
#   develop or main and weekly; on a pull request only where one of those
#   changed.
#
# Everything is on for a manual run, a scheduled one, a first push of a branch
# (nothing to compare against), and when the workflow itself changed.
set -e
out=${GITHUB_OUTPUT:-/dev/stdout}
all() {
    echo "unsafe=true" >>"$out"
    echo "floor=true" >>"$out"
    echo "arm64=true" >>"$out"
    exit 0
}
case "$EVENT" in
workflow_dispatch | schedule) all ;;
esac
case "$BASE" in
"" | 0000000000000000000000000000000000000000) all ;;
esac
changed=$(git diff --name-only "$BASE" HEAD) || all
echo "changed since $BASE:"
echo "$changed" | sed 's/^/  /'
if echo "$changed" | grep -q '^\.github/workflows/\|^scripts/ci-changes\.sh$'; then
    all
fi
if echo "$changed" | grep -q '^crates/unsafe/\|^scripts/check-unsafe-crates\.sh$'; then
    echo "unsafe=true" >>"$out"
else
    echo "unsafe=false" >>"$out"
fi
if [ "$EVENT" = "push" ] && { [ "$REF" = "refs/heads/main" ] || [ "$REF" = "refs/heads/develop" ]; }; then
    echo "floor=true" >>"$out"
elif echo "$changed" | grep -q 'Cargo\.toml$\|^Cargo\.lock$\|^rust-toolchain\.toml$\|^scripts/check-floor\.sh$'; then
    echo "floor=true" >>"$out"
elif git diff "$BASE" HEAD -- crates/nikaia/src/emit/mod.rs | grep -q '^[+-]pub const RUST_FLOOR'; then
    echo "floor=true" >>"$out"
else
    echo "floor=false" >>"$out"
fi
if [ "$EVENT" = "push" ] && { [ "$REF" = "refs/heads/main" ] || [ "$REF" = "refs/heads/develop" ]; }; then
    echo "arm64=true" >>"$out"
elif echo "$changed" | grep -q '^crates/nikaia-std/\|^crates/unsafe/\|Cargo\.toml$\|^Cargo\.lock$\|^rust-toolchain\.toml$'; then
    echo "arm64=true" >>"$out"
else
    echo "arm64=false" >>"$out"
fi
