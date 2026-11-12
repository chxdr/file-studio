#!/usr/bin/env bash
# Builds the Debug app and runs a debug-automation script in one go.
#
#   scripts/run-debug.sh "wait:1;key:cmd+l;type:~/Documents;key:return;wait:0.5;snap:docs;log;quit"
#   scripts/run-debug.sh --mock "go:/Mock/Huge 100k;bench:rows;snap:huge;quit"
#
# Options (before the script):
#   --mock          run against MockCore (no Rust)
#   --data <dir>    the core's data folder (default: a fresh temporary one,
#                   so runs never touch your real session)
#   --shots <dir>   where snap: writes PNGs (default: build/debug-shots)
#   --no-build      skip xcodegen/xcodebuild
# Prints the app's STEP/STATE/MENU/SNAP/BENCH lines; exits when the script quits
# (or after 120 s).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MOCK=() DATA="" SHOTS="$ROOT/build/debug-shots" BUILD=1
while [ $# -gt 1 ]; do
  case "$1" in
    --mock) MOCK=(-FSMockCore); shift ;;
    --data) DATA="$2"; shift 2 ;;
    --shots) SHOTS="$2"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    *) break ;;
  esac
done
SCRIPT="${1:?usage: run-debug.sh [--mock] [--data dir] [--shots dir] [--no-build] \"step;step;…\"}"
[ -z "$DATA" ] && DATA="$(mktemp -d "${TMPDIR:-/tmp}/fstudio-debug.XXXXXX")"
mkdir -p "$SHOTS"

cd "$ROOT/apple"
if [ "$BUILD" = 1 ]; then
  xcodegen -q
  xcodebuild -project FileStudio.xcodeproj -scheme "File Studio" -configuration Debug \
    -derivedDataPath build/DerivedData build -quiet 2>&1 | grep -E "error|BUILD FAILED" && exit 1 || true
fi
# Debug builds are "File Studio Debug" (bundle id app.filestudio.mac.debug), so
# they never share preferences or data with the release app.
APP="$ROOT/apple/build/DerivedData/Build/Products/Debug/File Studio Debug.app/Contents/MacOS/File Studio Debug"
echo "data: $DATA  shots: $SHOTS"
# NSUserDefaults reads -Key value pairs from the arguments.
"$APP" ${MOCK[@]+"${MOCK[@]}"} -FSDataDir "$DATA" -FSDebugSnapshotDir "$SHOTS" -FSDebugScript "$SCRIPT" \
  -ApplePersistenceIgnoreState YES 2>/dev/null &
PID=$!
( sleep 120; kill "$PID" 2>/dev/null ) >/dev/null 2>&1 &
WATCHDOG=$!
wait "$PID" || true
pkill -P "$WATCHDOG" 2>/dev/null || true
kill "$WATCHDOG" 2>/dev/null || true
