#!/bin/zsh
# Create (or re-attach) an APFS sparse image used as a "second volume" for spikes. Usage: mkimage.sh <workdir> [case-sensitive]
set -e
W=$1; FS=${2:-APFS}; NAME=FSSpike${3:-}
[ -f $W/$NAME.sparseimage ] || hdiutil create -size 40g -fs "$FS" -type SPARSE -volname $NAME $W/$NAME.sparseimage >/dev/null
hdiutil attach $W/$NAME.sparseimage -nobrowse >/dev/null
echo /Volumes/$NAME
