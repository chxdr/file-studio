#!/bin/zsh
# Safe Transfer M2 benchmarks onto the internal SSD, an APFS image and an
# exFAT image (private mount points, deleted on exit):
# - small files (proposal §4.7): 5,000 files of 1-64 KiB, verified, through
#   the engine's group commit, 3 runs; plus the per-file-flush baseline
#   (1,000 files). Gate: APFS image <= 2.4 s.
# - one 1 GiB file through the engine (64 MiB chunks, flush per chunk,
#   uncached read-back) against `cp` + `sync`, 2 runs.
# Disk images are files on the internal SSD, cached by the host, so their
# MB/s are not card or USB speeds; compare rows, not absolute numbers.
set -e
ROOT=${0:A:h:h:h}
W=$(mktemp -d -t st-bench)
cleanup() {
  for m in $W/apfs $W/exfat; do hdiutil detach -quiet -force $m 2>/dev/null || true; done
  rm -rf "${W:?}"
}
trap cleanup EXIT
cd $ROOT
cargo build -q --release -p safe-transfer --example bench_small --example bench_big
B=$ROOT/target/release/examples/bench_small
BIG=$ROOT/target/release/examples/bench_big
mkdir -p $W/apfs $W/exfat $W/ssd
hdiutil create -quiet -megabytes 3072 -fs APFS -volname STBENCH $W/apfs.dmg
hdiutil create -quiet -megabytes 3072 -fs ExFAT -volname STBENCH $W/exfat.dmg
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

for d in ssd apfs exfat; do rm -rf "${W:?}/$d"/g* "${W:?}/$d/p"; done
head -c 1073741824 /dev/urandom > $W/big.bin
for run in 1 2; do
  for d in ssd apfs exfat; do
    mkdir -p $W/$d/cp$run $W/$d/st$run
    t0=$(perl -MTime::HiRes=time -e 'print time')
    cp $W/big.bin $W/$d/cp$run/ && sync
    t1=$(perl -MTime::HiRes=time -e 'print time')
    echo "$d 1GiB cp+sync run$run: $(perl -e "printf '%.3f', $t1-$t0") s"
    rm -rf "${W:?}/$d/cp$run"
    echo -n "$d 1GiB safe-transfer run$run: "; $BIG $W/big.bin $W/$d/st$run
    rm -rf "${W:?}/$d/cp$run" "${W:?}/$d/st$run"
  done
done
