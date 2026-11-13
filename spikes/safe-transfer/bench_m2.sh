#!/bin/zsh
# Safe Transfer M2 small-file gate (proposal §4.7): 5,000 files of 1-64 KiB,
# verified, through the engine's group commit, onto the internal SSD, an
# APFS image and an exFAT image; plus the per-file-flush baseline (1,000
# files). Images are mounted at private mount points and deleted on exit.
# Gate: APFS image <= 2.4 s.
set -e
ROOT=${0:A:h:h:h}
W=$(mktemp -d -t st-bench)
cleanup() {
  for m in $W/apfs $W/exfat; do hdiutil detach -quiet -force $m 2>/dev/null || true; done
  rm -rf "${W:?}"
}
trap cleanup EXIT
cd $ROOT
cargo build -q --release -p safe-transfer --example bench_small
B=$ROOT/target/release/examples/bench_small
mkdir -p $W/apfs $W/exfat $W/ssd
hdiutil create -quiet -megabytes 2048 -fs APFS -volname STBENCH $W/apfs.dmg
hdiutil create -quiet -megabytes 2048 -fs ExFAT -volname STBENCH $W/exfat.dmg
hdiutil attach -quiet -nobrowse -mountpoint $W/apfs $W/apfs.dmg
hdiutil attach -quiet -nobrowse -mountpoint $W/exfat $W/exfat.dmg
for run in 1 2 3; do
  for d in ssd apfs exfat; do
    mkdir $W/$d/g$run; echo -n "$d group run$run: "; $B $W/$d/g$run 5000
  done
done
for d in ssd apfs exfat; do
  mkdir $W/$d/p; echo -n "$d per-file: "; $B $W/$d/p 1000 --per-file
done
