#!/usr/bin/env bash
# ADR-289's number: what it costs to answer a request with a file, five ways -
# including the two that never read it. See README.md for the method and
# docs/history/zero-copy-send.md for what the numbers settled.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
repeats=${1:-5}

cargo build --manifest-path "$root/benches/sendfile/Cargo.toml" --release --quiet
exec "$root/target/release/sendfile" "$repeats"
