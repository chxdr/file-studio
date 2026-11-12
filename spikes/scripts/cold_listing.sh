#!/bin/zsh
# Cold-cache listing: detach+reattach the image volume (drops its cached metadata) before each run.
# Usage: cold_listing.sh <workdir> <cycles> ; expects /Volumes/FSSpike/{d10000,d100000}
W=$1; N=$2; BIN=$(dirname $0)/../target/release/s1_listing
for size in 10000 100000; do for m in ${(@)METHODS:-"std read_dir+symlink" "std read_dir+DirEntry" "getattrlistbulk 256K" "getattrlistbulk adaptive" fts readdir}; do
  for i in $(seq $N); do
    hdiutil detach /Volumes/FSSpike >/dev/null 2>&1; hdiutil attach $W/FSSpike.sparseimage -nobrowse >/dev/null
    $BIN run /Volumes/FSSpike/d$size 1 "$m" | grep '^| [a-z]' | grep -v '^| method' | sed "s/^/COLD $i /"
  done
done; done
