#!/bin/zsh
# Reproduces SAFE-TRANSFER-PROPOSAL §4.7: small-file group-commit cost, and the
# yank harness (kill -9 the image's diskimages-helper) with its negative controls.
# Makes 1 GB APFS and exFAT images in a temporary folder; detaches and deletes them.
set -e
HERE=${0:A:h}
W=$(mktemp -d -t st-batch)
P=$W/st_probe
clang -O2 -Wall -o $P $HERE/st_probe.c
cleanup() {
  for v in /Volumes/STBA /Volumes/STBX; do hdiutil detach -quiet -force $v 2>/dev/null || true; done
  rm -rf "${W:?}"
}
trap cleanup EXIT
cd $W
hdiutil create -quiet -megabytes 1024 -fs APFS -volname STBA ba.dmg 2>/dev/null
hdiutil create -quiet -megabytes 1024 -fs ExFAT -volname STBX bx.dmg 2>/dev/null
hdiutil attach -quiet -nobrowse ba.dmg; hdiutil attach -quiet -nobrowse bx.dmg
mkdir -p $W/ssd

echo "== cost: 5,000 files of 1-64 KiB, with read-back, rename and folder flush"
for v in $W/ssd /Volumes/STBA /Volumes/STBX; do echo "-- $v"
  for cfg in "1 none 1" "1 perfile_full 1" "64 fsync_group 1" "256 fsync_group 1" "1024 fsync_group 1"; do
    set -- ${=cfg}; mkdir $v/b; $P batch $v/b 5000 $1 $2 $3; rm -rf "${v:?}/b"
  done
done

echo "== yank harness: 2,000 files written, then kill -9 the image's helper"
helper() { hdiutil info | awk -v img="$W/$1.dmg" '$1=="image-path"{cur=($3==img)} cur && $1=="process" {print $4; exit}'; }
for pair in "ba STBA" "bx STBX"; do set -- ${=pair}; v=/Volumes/$2
  for mode in cached_none nocache_none fsync_each cached_group_full nocache_group_full; do
    [ -d $v ] || hdiutil attach -quiet -nobrowse $1.dmg
    mkdir -p $v/y; h=$(helper $1)
    $P batchwrite $v/y 2000 $mode >/dev/null; kill -9 $h
    sleep 1; hdiutil detach -quiet -force $v 2>/dev/null || true; diskutil unmount force $v >/dev/null 2>&1 || true; sleep 1
    hdiutil attach -quiet -nobrowse $1.dmg 2>/dev/null || { sleep 2; hdiutil attach -quiet -nobrowse $1.dmg; }
    echo -n "$2 $mode: "; $P batchcheck $v/y 2000; rm -rf "${v:?}/y"
  done
done
