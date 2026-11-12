#!/usr/bin/env bash
# Debug-automation proofs for docs/SAFETY-AUDIT.md, run in the real Debug app
# against throwaway scratch homes (never your real folders).
#
#   scripts/safety-proofs.sh            build once, then run every proof
#   scripts/safety-proofs.sh s1 s6      only these
#   scripts/safety-proofs.sh --no-build s4
#
# Each proof builds a fresh scratch home under $TMPDIR, runs a script with
# HOME pointing at it, checks the app's printed lines (and the disk), prints
# PASS or FAIL, and deletes the scratch home.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD=1
if [ "${1:-}" = "--no-build" ]; then BUILD=0; shift; fi
WANT=("$@")
[ ${#WANT[@]} -eq 0 ] && WANT=(s1 s4 s5 s6 s7 s11)
if [ "$BUILD" = 1 ]; then
  (cd "$ROOT/apple" && xcodegen -q && xcodebuild -project FileStudio.xcodeproj -scheme "File Studio" \
    -configuration Debug -derivedDataPath build/DerivedData build -quiet 2>&1 | grep -E "error:|BUILD FAILED") && exit 1
fi
FAILS=0

# run <name> <script>: runs the app with HOME=$H; output in $OUT.
run() {
  OUT="$(HOME="$H" "$ROOT/scripts/run-debug.sh" --no-build --shots "$H/.shots" --trash "$H/.Trash" "$2" 2>&1)"
  echo "$OUT" | grep -E "^(STEP|CONFIRM|LS|TOAST|MOVED|KEY|PASTE|LIST row|STATUS)" > "$H/.log" || true
}
# expect <description> <grep pattern>: the output has a matching line.
expect() {
  if echo "$OUT" | grep -qE "$2"; then echo "  ok   $1"; else echo "  FAIL $1 (no line matching: $2)"; BAD=1; fi
}
# refuse <description> <grep pattern>: the output has no matching line.
refuse() {
  if echo "$OUT" | grep -qE "$2"; then echo "  FAIL $1 (found: $(echo "$OUT" | grep -E "$2" | head -1))"; BAD=1; else echo "  ok   $1"; fi
}
home() {
  H="$(mktemp -d "${TMPDIR:-/tmp}/fstudio-safety.XXXXXX")"
  H="$(cd "$H" && pwd -P)"
  mkdir -p "$H/Library" "$H/.Trash"
  BAD=0
}
finish() {
  if [ "$BAD" = 0 ]; then echo "PASS $1"; else echo "FAIL $1 (log: $H/.log)"; FAILS=$((FAILS + 1)); return; fi
  rm -rf "$H"
}

# S1: the Delete Immediately sheet names junk.tmp; a paste lands and the
# selection changes while it is up; Yes deletes junk.tmp only.
s1() {
  home
  mkdir -p "$H/work" "$H/other"
  echo junk > "$H/work/junk.tmp"
  head -c 2000000 /dev/urandom > "$H/other/thesis.pdf"
  run s1 "wait:1;set:operations.allow_delete_immediately=true;go:$H/work;wait:1;select:junk.tmp;key:cmd+alt+delete;wait:0.7;op:copy:$H/other/thesis.pdf:$H/work;wait:1.5;select:junk.tmp|thesis.pdf;listlog;confirm:yes;wait:1;ls:$H/work;quit"
  expect "the sheet names junk.tmp only" "^CONFIRM Delete “junk.tmp” immediately\? \| junk.tmp$"
  expect "both rows selected when Yes is pressed" "^LIST row 1 \[x\]  thesis.pdf"
  expect "only junk.tmp was deleted" "^LS .*/work: thesis.pdf$"
  finish s1
}

# S4: ⌘V 3,000 files, then click mine.txt at once. When the paste has landed
# the selection is still just mine.txt (the pending selection was cancelled).
s4() {
  home
  mkdir -p "$H/work" "$H/other"
  echo mine > "$H/work/mine.txt"
  for i in $(seq 1 3000); do echo "$i" > "$H/other/f$i.txt"; done
  run s4 "wait:1;go:$H/other;wait:1;focus:list;key:cmd+a;key:cmd+c;go:$H/work;wait:1;key:cmd+v;select:mine.txt;wait:5;sel;quit"
  expect "the selection is still mine.txt alone" "^SEL 1: mine.txt$"
  local n; n=$(ls "$H/work" | wc -l | tr -d ' ')
  if [ "$n" = 3001 ]; then echo "  ok   the paste landed (3,001 items)"; else echo "  FAIL the paste has $n items"; BAD=1; fi
  finish s4
}

# S5: rename Notes.txt to NOTES.txt in place. The list shows 3 rows (no ghost
# of the old name), matching the disk.
s5() {
  home
  mkdir -p "$H/w"
  echo a > "$H/w/Notes.txt"; echo b > "$H/w/a.txt"; echo c > "$H/w/b.txt"
  run s5 "wait:1;go:$H/w;wait:1;focus:list;select:Notes.txt;key:return;wait:0.5;key:cmd+a;type:NOTES.txt;key:return;wait:3;listlog;ls:$H/w;quit"
  expect "the disk has NOTES.txt" "^LS .*/w: NOTES.txt, a.txt, b.txt$"
  expect "the list has 3 rows" "^LIST mode=.* rows=3 "
  expect "the row is NOTES.txt" "^LIST row 2 \\[x\\]> NOTES.txt"
  refuse "no ghost row" "^LIST row .*Notes.txt"
  finish s5
}

# S6: hold ⌘⌫ (one press + 4 auto-repeats): one item goes to the (scratch)
# Trash. Hold ⌘D: one duplicate. Hold ↓ (1 + 3 repeats): the cursor moves 4.
s6() {
  home
  mkdir -p "$H/w"
  for n in a1 a2 a3 a4 a5 a6 a7; do echo "$n" > "$H/w/$n"; done
  run s6 "wait:1;go:$H/w;wait:1;focus:list;select:a1;holdkey:cmd+delete:4;wait:1.5;ls:$H/w;ls:$H/.Trash;holdkey:cmd+d:4;wait:1.5;ls:$H/w;select:a2;holdkey:down:3;wait:0.5;sel;quit"
  expect "the repeats reached the app" "^KEY repeat .*delete"
  expect "held ⌘⌫ trashed a1 only" "^LS .*/\\.Trash: a1$"
  expect "held ⌘D duplicated once" "^LS .*/w: a2, a2 copy, a3, a4, a5, a6, a7$"
  expect "held ↓ moved four rows" "^SEL 1: a5$"
  finish s6
}

for p in "${WANT[@]}"; do
  if declare -F "$p" >/dev/null; then echo "── $p"; "$p"; else echo "no proof named $p"; fi
done
[ "$FAILS" = 0 ] && echo "all proofs passed" || { echo "$FAILS proof(s) failed"; exit 1; }
