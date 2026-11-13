#!/usr/bin/env bash
# Debug-automation proof for live workspaces, run in the real Debug app
# against a throwaway scratch home (never your real folders).
#
#   scripts/workspace-proofs.sh              build, then run
#   scripts/workspace-proofs.sh --no-build
#
# It creates "Photo work" through the New Workspace sheet with its own tabs
# and dual pane, switches back and forth with ⌃⌘1/⌃⌘2, relaunches to check
# both come back, then deletes one through the core's question. Screenshots
# land in build/workspace-shots. Prints PASS or FAIL.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD=1
[ "${1:-}" = "--no-build" ] && BUILD=0
if [ "$BUILD" = 1 ]; then
  (cd "$ROOT/apple" && xcodegen -q && xcodebuild -project FileStudio.xcodeproj -scheme "File Studio" \
    -configuration Debug -derivedDataPath build/DerivedData build -quiet 2>&1 | grep -E "error:|BUILD FAILED") && exit 1
fi
SHOTS="$ROOT/build/workspace-shots"
rm -rf "$SHOTS"; mkdir -p "$SHOTS"
H="$(mktemp -d "${TMPDIR:-/tmp}/fstudio-ws.XXXXXX")"; H="$(cd "$H" && pwd -P)"
DATA="$H/.data"
mkdir -p "$H/Library" "$H/.Trash" "$H/Documents/Letters" "$H/Pictures/Raw" "$H/Pictures/Edits" "$DATA"
for i in 1 2 3; do echo "$i" > "$H/Pictures/Raw/IMG_000$i.CR3"; done
echo x > "$H/Pictures/Edits/IMG_0001.jpg"; echo y > "$H/Documents/notes.txt"
BAD=0

run() {
  OUT="$(HOME="$H" "$ROOT/scripts/run-debug.sh" --no-build --data "$DATA" --shots "$SHOTS" --trash "$H/.Trash" "$1" 2>&1)"
  # An app that never started its script (a launch hiccup right after another
  # run) printed no STEP line; nothing happened, so try once more.
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
# expect_seq <description> <pattern>…: lines matching each pattern appear in this order.
expect_seq() {
  local what="$1"; shift
  local rest="$OUT" p
  for p in "$@"; do
    local n; n=$(echo "$rest" | grep -nE "$p" | head -1 | cut -d: -f1)
    if [ -z "$n" ]; then echo "  FAIL $what (missing, in order: $p)"; BAD=1; return; fi
    rest="$(echo "$rest" | tail -n +"$((n + 1))")"
  done
  echo "  ok   $what"
}

echo "── workspaces: create, switch, screenshots"
run "wait:1;go:$H/Documents;wait:1;workspaces;wsmenu;\
perform:workspace.save:menu;wait:0.8;snap:1-new-workspace-sheet;wsform:Photo work|camera|empty;wscreate;wait:1;\
go:$H/Pictures/Raw;wait:0.6;newtab:$H/Pictures/Edits;wait:0.6;perform:pane.toggle_dual;wait:0.6;layout:horizontal,0.55;wait:0.8;\
workspaces;wsmenu;snap:2-photo-work;\
key:ctrl+cmd+1;wait:1.2;workspaces;snap:3-everyday;\
key:ctrl+cmd+2;wait:1.2;workspaces;snap:4-photo-work-again;\
perform:workspace.manage:menu;wait:1;snap:5-manage-sheet;wsclose;wait:0.5;quit"
expect "first launch has Everyday, active, on ⌃⌘1" "^WORKSPACE 0 Everyday icon=house tabs=1 windows=1 active key=⌃⌘1$"
expect "the sheet created Photo work" "^WSCREATE ok$"
expect "Photo work is second, with the camera icon and ⌃⌘2" "^WORKSPACE 1 Photo work icon=camera tabs=3 windows=1 active key=⌃⌘2$"
expect "the switcher lists both, Photo work ticked" "^WSMENU \[x\] Photo work$"
n=$(echo "$OUT" | grep -cE "^WSMENU \\[.\\] .*Photo work$")
if [ "$n" = 1 ]; then echo "  ok   the open switcher lists Photo work once"; else echo "  FAIL the open switcher lists Photo work $n times"; BAD=1; fi
expect "the Workspaces menu lists Photo work on ⌃⌘2" "^MAINMENU Workspaces: \[x\] Photo work key=⌃⌘2$"
expect_seq "⌃⌘1 / ⌃⌘2 bring each layout back" \
  "^STEP key:ctrl\+cmd\+1" "^WSLAYOUT window panes=1 tabs=1 .*locations=Documents$" \
  "^STEP key:ctrl\+cmd\+2" "^WSLAYOUT window panes=2 tabs=2\+1 orientation=horizontal ratio=0.55 .*locations=Raw,Edits,"
expect_seq "Everyday was saved when leaving it" \
  "^STEP key:ctrl\+cmd\+1" "^WORKSPACE 0 Everyday icon=house tabs=1 windows=1 active"

echo "── workspaces: relaunch restores both"
run "wait:1.2;workspaces;snap:6-after-relaunch;key:ctrl+cmd+1;wait:1.2;workspaces;quit"
expect_seq "Photo work comes back active, then Everyday on ⌃⌘1" \
  "^WORKSPACE 1 Photo work .* active" "^WSLAYOUT window panes=2 tabs=2\+1 .*locations=Raw,Edits," \
  "^STEP key:ctrl\+cmd\+1" "^WSLAYOUT window panes=1 tabs=1 .*locations=Documents$"

echo "── workspaces: delete asks first, touches no files"
run "wait:1.2;wsdelete:1;wait:0.8;snap:7-delete-question;confirm:yes;wait:1;workspaces;wsdelete:0;wait:0.5;quit"
expect "the question names Photo work" "^CONFIRM Delete the workspace “Photo work”\? \| Cancel / Delete Workspace$"
expect_seq "only Everyday is left, active" "^STEP confirm:yes" "^WORKSPACE 0 Everyday .* active"
expect "the last one can't be deleted" "^WSDELETE error: The last workspace can.t be deleted\.$"
if [ "$(ls "$H/Pictures/Raw" | wc -l | tr -d ' ')" = 3 ] && [ -f "$H/Pictures/Edits/IMG_0001.jpg" ] && [ -f "$H/Documents/notes.txt" ]; then
  echo "  ok   every file is still there"
else
  echo "  FAIL files changed"; BAD=1
fi

echo "shots: $SHOTS"
if [ "$BAD" = 0 ]; then echo "PASS workspaces"; rm -rf "$H"; else echo "FAIL workspaces (log: $H/.log)"; exit 1; fi
