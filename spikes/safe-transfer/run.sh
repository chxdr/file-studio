#!/bin/zsh
# Reproduces the Safe Transfer proposal measurements (docs/SAFE-TRANSFER-PROPOSAL.md §12).
# Makes 3 GB APFS and exFAT disk images and a small FAT one in a temporary folder,
# runs st_probe on them, then detaches and deletes everything.
# Usage: spikes/safe-transfer/run.sh
set -e
HERE=${0:A:h}
W=$(mktemp -d -t st-probe)
P=$W/st_probe
clang -O2 -Wall -o $P $HERE/st_probe.c
cleanup() {
  for v in /Volumes/STProbeAPFS /Volumes/STX /Volumes/STF; do hdiutil detach -quiet -force $v 2>/dev/null || true; done
  rm -rf "${W:?}"
}
trap cleanup EXIT
hdiutil create -quiet -megabytes 3072 -fs APFS -volname STProbeAPFS $W/apfs.dmg 2>/dev/null
hdiutil create -quiet -megabytes 3072 -fs ExFAT -volname STX $W/exfat.dmg 2>/dev/null
hdiutil create -quiet -megabytes 256 -fs MS-DOS -volname STF $W/fat.dmg 2>/dev/null
for i in apfs exfat fat; do hdiutil attach -quiet -nobrowse $W/$i.dmg 2>/dev/null; done

for v in /Volumes/STProbeAPFS /Volumes/STX /Volumes/STF; do
  echo "== $v"; $P volid $v; $P fullfsync $v
done

echo "== volume identity across a re-attach (exFAT)"
hdiutil detach -quiet /Volumes/STX; hdiutil detach -quiet /Volumes/STF
hdiutil attach -quiet -nobrowse $W/fat.dmg 2>/dev/null; hdiutil attach -quiet -nobrowse $W/exfat.dmg 2>/dev/null
$P volid /Volumes/STX; $P volid /Volumes/STF

for v in /Volumes/STProbeAPFS /Volumes/STX; do
  echo "== flush cost, $v (20 x 50 MiB, then 1 GiB in 64 / 16 MiB chunks)"
  for cfg in "0 none" "1 none" "1 fsync" "1 full" "0 full"; do
    set -- ${=cfg}; $P write $v 20 50 64 $1 $2; rm -f $v/f*.bin(N)
  done
  $P write $v 1 1024 64 1 full; rm -f $v/f*.bin(N)
  $P write $v 1 1024 64 1 chunk; rm -f $v/f*.bin(N)
  $P write $v 1 1024 16 1 chunk; rm -f $v/f*.bin(N)
  echo "== is the read-back served from RAM? $v"
  $P write $v 20 50 64 0 full >/dev/null; $P readback $v 20 1; rm -f $v/f*.bin(N)
  $P write $v 20 50 64 1 full >/dev/null; $P readback $v 20 1; rm -f $v/f*.bin(N)
  $P write $v 1 50 64 0 full >/dev/null; $P evict $v/f0000.bin; $P readback $v 1 1; rm -f $v/f*.bin(N)
done
