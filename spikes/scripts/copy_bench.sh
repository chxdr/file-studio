#!/bin/zsh
# usage: copy_bench.sh <workdir> <dst-dir-same-volume> <dst-dir-other-volume>
W=$1; SAME=$2; OTHER=$3; B=$(dirname $0)/../target/release/s3_copy
[ -f $W/big1g ] || $B gen-big $W/big1g 1073741824
[ -f $W/big10g ] || $B gen-big $W/big10g 10737418240
mkdir -p $SAME $OTHER
M1=(copyfile_all copyfile_all_clone clonefile rw:64:n rw:256:n rw:1024:n rw:4096:n rw:8192:n rw:1024:d rw:4096:d rw:1024:b rw:4096:b rw:1024:s cp cp-c ditto)
for tgt in same other; do
  D=$SAME; [ $tgt = other ] && D=$OTHER
  echo "### 1 GiB -> $tgt volume (method | median ms | p95 ms | median MB/s | p95-run MB/s)"
  for m in $M1; do $B bench $m $W/big1g $D 5; done
done
M10=(copyfile_all copyfile_all_clone rw:1024:n rw:4096:d rw:4096:b cp ditto)
for tgt in same other; do
  D=$SAME; [ $tgt = other ] && D=$OTHER
  echo "### 10 GiB -> $tgt volume"
  for m in $M10; do $B bench $m $W/big10g $D 3; done
done
