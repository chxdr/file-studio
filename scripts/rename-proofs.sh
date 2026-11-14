#!/usr/bin/env bash
# Debug-automation proof for batch rename (M3), run in the real Debug app
# against a throwaway scratch home (never your real folders).
#
#   scripts/rename-proofs.sh              build, then run
#   scripts/rename-proofs.sh --no-build
#
# It opens "Batch Rename N Items…" with ⌃⌘R, checks the menus, previews a
# template, applies it, undoes it with one ⌘Z, swaps two names, reads EXIF
# tokens from a DNG, shows collisions with the Rename button off, and saves
# a preset that is still there after a relaunch. Screenshots land in
# build/rename-shots. Prints PASS or FAIL.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD=1
[ "${1:-}" = "--no-build" ] && BUILD=0
if [ "$BUILD" = 1 ]; then
  (cd "$ROOT/apple" && xcodegen -q && xcodebuild -project FileStudio.xcodeproj -scheme "File Studio" \
    -configuration Debug -derivedDataPath build/DerivedData build -quiet 2>&1 | grep -E "error:|BUILD FAILED") && exit 1
fi
SHOTS="$ROOT/build/rename-shots"
rm -rf "$SHOTS"; mkdir -p "$SHOTS"
H="$(mktemp -d "${TMPDIR:-/tmp}/fstudio-rename.XXXXXX")"; H="$(cd "$H" && pwd -P)"
DATA="$H/.data"
mkdir -p "$H/Library" "$H/.Trash" "$H/Trip" "$H/Swap" "$H/Exif" "$H/Clash" "$DATA"
for i in 1 2 3 4 5; do echo "photo $i" > "$H/Trip/IMG_000$i.JPG"; done
echo first > "$H/Swap/1.txt"; echo second > "$H/Swap/2.txt"
echo x > "$H/Clash/x.txt"; echo y > "$H/Clash/y.txt"; echo keep > "$H/Clash/keep.txt"
# A minimal TIFF-based raw with Make, Model, DateTimeOriginal and LensModel.
python3 -I - "$H/Exif/shot.dng" <<'PY'
import struct, sys
def z(s): return s.encode() + b"\0"
make, model, date, lens = z("Canon"), z("Canon EOS R5"), z("2024:06:01 14:03:22"), z("RF24-70mm F2.8")
ifd0 = 8; ifd0_len = 2 + 4 * 12 + 4; exif = ifd0 + ifd0_len; exif_len = 2 + 2 * 12 + 4
off = exif + exif_len; data = b""; offs = []
for s in (make, model, date, date, lens):
    offs.append(off + len(data)); data += s
e = lambda tag, typ, n, v: struct.pack("<HHII", tag, typ, n, v)
b = b"II*\0" + struct.pack("<I", ifd0) + struct.pack("<H", 4)
b += e(0x010F, 2, len(make), offs[0]) + e(0x0110, 2, len(model), offs[1]) + e(0x0132, 2, len(date), offs[2]) + e(0x8769, 4, 1, exif)
b += struct.pack("<I", 0) + struct.pack("<H", 2) + e(0x9003, 2, len(date), offs[3]) + e(0xA434, 2, len(lens), offs[4]) + struct.pack("<I", 0) + data
open(sys.argv[1], "wb").write(b)
PY
BAD=0

run() {
  OUT="$(HOME="$H" "$ROOT/scripts/run-debug.sh" --no-build --data "$DATA" --shots "$SHOTS" --trash "$H/.Trash" "$1" 2>&1)"
  if ! echo "$OUT" | grep -q "^STEP"; then
    echo "  (the app ran no steps; launching again)"
    sleep 2
    OUT="$(HOME="$H" "$ROOT/scripts/run-debug.sh" --no-build --data "$DATA" --shots "$SHOTS" --trash "$H/.Trash" "$1" 2>&1)"
  fi
  echo "$OUT" >> "$H/.log"
}
expect() {
  if echo "$OUT" | grep -qE "$2"; then echo "  ok   $1"; else echo "  FAIL $1 (no line matching: $2)"; BAD=1; fi
}
expect_files() {
  local got; got="$(ls "$2" | tr '\n' '|')"
  if [ "$got" = "$3" ]; then echo "  ok   $1"; else echo "  FAIL $1 (got $got, want $3)"; BAD=1; fi
}
no_temps() {
  if find "$H" -name '*fstudio-renaming*' -o -name '*fstudio-partial*' | grep -q .; then
    echo "  FAIL temporary names left behind"; BAD=1
  else echo "  ok   no temporary names left anywhere"; fi
}

echo "── menus, ⌃⌘R, preview, apply, one-step undo"
run "wait:1;go:$H/Trip;wait:1;menus;key:cmd+a;wait:0.3;ctx:0;key:ctrl+cmd+r;wait:1;brename:show;\
brename:set:0:template=Trip {n:3};brename:add:ext;brename:set:1:extChange=lower;wait:0.3;brename:show;brename:rows;\
snapv:1-trip-preview;brename:apply;wait:1.5;log;undo;wait:1.5;log;quit"
expect "File menu has Batch Rename…" "MAINMENU File › Batch Rename…"
expect "multi-selection menu says Batch Rename 5 Items…" "Batch Rename 5 Items…"
expect "⌃⌘R opens the sheet for the 5 selected items" "BRSUM total=5"
expect "template + lower-case extension previews" "BRROW 2 ready IMG_0003.JPG -> Trip 003.jpg"
expect "the button counts what will be renamed" 'button="Rename 5 Items" enabled=true'
echo "$OUT" | grep -E "^(MAINMENU File › Batch|BRSUM|BRROW 0|SNAP)" | sed 's/^/       /'
expect_files "after undo every name is back" "$H/Trip" "IMG_0001.JPG|IMG_0002.JPG|IMG_0003.JPG|IMG_0004.JPG|IMG_0005.JPG|"

echo "── apply stays applied (no undo)"
run "wait:1;go:$H/Trip;wait:1;activate;wait:0.2;key:cmd+a;key:ctrl+cmd+r;wait:1;brename:show;brename:apply;wait:1.5;snap:2-renamed;quit"
expect "the sheet reopens with the last steps" "BRSTEP 0 template enabled=true template=Trip \{n:3\}"
expect_files "renamed on disk" "$H/Trip" "Trip 001.jpg|Trip 002.jpg|Trip 003.jpg|Trip 004.jpg|Trip 005.jpg|"
[ "$(cat "$H/Trip/Trip 003.jpg")" = "photo 3" ] && echo "  ok   contents followed their names" || { echo "  FAIL contents"; BAD=1; }

echo "── a swap (1 ↔ 2) through temporary names"
run "wait:1;go:$H/Swap;wait:1;activate;wait:0.2;key:cmd+a;key:ctrl+cmd+r;wait:1;brename:remove:1;brename:set:0:template={n:1:2:-1};wait:0.3;brename:rows;brename:apply;wait:1.5;quit"
expect "the swap previews as two ready rows" "BRROW 1 ready 2.txt -> 1.txt"
[ "$(cat "$H/Swap/1.txt")" = "second" ] && [ "$(cat "$H/Swap/2.txt")" = "first" ] \
  && echo "  ok   swapped on disk" || { echo "  FAIL swap: 1.txt=$(cat "$H/Swap/1.txt") 2.txt=$(cat "$H/Swap/2.txt")"; BAD=1; }

echo "── EXIF tokens"
run "wait:1;go:$H/Exif;wait:1;activate;wait:0.2;key:cmd+a;key:ctrl+cmd+r;wait:1;brename:set:0:template={exif.date:%Y%m%d}_{exif.camera}_{exif.lens};wait:1.5;brename:show;brename:rows;snapv:3-exif;brename:cancel;quit"
expect "EXIF date, camera and lens fill the name" "BRROW 0 ready shot.dng -> 20240601_Canon EOS R5_RF24-70mm F2.8.dng"

echo "── problems: collisions turn the button off"
run "wait:1;go:$H/Clash;wait:1;activate;wait:0.2;key:cmd+a;key:ctrl+cmd+r;wait:1;brename:set:0:template=keep;wait:0.3;brename:show;brename:rows;snapv:4-problems;\
brename:set:0:template={name}/bad;wait:0.3;brename:rows;brename:set:0:template={nope};wait:0.3;brename:show;snapv:5-bad-token;brename:cancel;quit"
expect "two items asking for the same name are flagged" "BRROW 1 problem x.txt -> keep.txt \[another item gets the same name\]"
expect "nothing to rename: the button is off" 'ready=0 .*button="Rename 0 Items" enabled=false'
expect "a / in a name is flagged" "names can't contain “/”"
expect "an unknown token is explained" "error=step 1: unknown token|error=unknown token"
expect_files "nothing changed on disk" "$H/Clash" "keep.txt|x.txt|y.txt|"

echo "── presets survive a relaunch"
run "wait:1;go:$H/Trip;wait:1;activate;wait:0.2;key:cmd+a;key:ctrl+cmd+r;wait:1;brename:set:0:template={parent} {n:2};brename:save:My Trip;brename:presets;brename:cancel;quit"
expect "saved" "BRPRESET My Trip steps=1"
run "wait:1;go:$H/Trip;wait:1;activate;wait:0.2;key:cmd+a;key:ctrl+cmd+r;wait:1;brename:presets;brename:load:My Trip;brename:rows:1;brename:cancel;quit"
expect "still there after a relaunch, and it loads" "BRROW 0 ready Trip 001.jpg -> Trip 01.jpg"
expect "listed" "BRPRESET My Trip steps=1"

no_temps
echo "shots: $SHOTS   log: $H/.log"
if [ "$BAD" = 0 ]; then echo PASS; rm -rf "$H"; else echo FAIL; exit 1; fi
