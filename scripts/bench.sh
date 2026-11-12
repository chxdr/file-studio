#!/bin/sh
# Runs fstudio-bench in release mode and appends the results to bench/history.md.
# Extra arguments go to fstudio-bench (--large, --quick, --label M2, --json out.json).
set -eu
cd "$(dirname "$0")/.."
exec cargo run --release -p fstudio-bench -- --record "$@"
