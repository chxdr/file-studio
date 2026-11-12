#!/usr/bin/env bash
# Builds the Rust core for the macOS app (F-004, Import Studio's D-024):
#   1. the static library  target/aarch64-apple-darwin/<profile>/libfstudio_ffi.a
#   2. UniFFI Swift bindings  apple/FileStudio/Core/Generated/fstudio_ffi.swift
#      (rewritten only when they change, so Xcode doesn't recompile needlessly)
#   3. the C header and module map  target/uniffi-headers/
# The app links the static library directly and reads the module map from
# target/uniffi-headers; there is no XCFramework snapshot to go stale.
#
# Usage: scripts/build-core-apple.sh [release|debug]   (default: release)
# Xcode's RustCore target runs this before the app builds.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${1:-release}"
TARGET=aarch64-apple-darwin
export MACOSX_DEPLOYMENT_TARGET=26.0
# Xcode's environment doesn't load the user's shell profile.
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
# Xcode exports SDK variables meant for Swift; cargo picks the host SDK itself.
unset SDKROOT

cd "$ROOT"
if [ "$PROFILE" = "release" ]; then
  cargo build -p fstudio-ffi --release --target "$TARGET"
  OUT="target/$TARGET/release"
else
  cargo build -p fstudio-ffi --target "$TARGET"
  OUT="target/$TARGET/debug"
fi

GEN="$ROOT/target/uniffi-swift"
rm -rf "$GEN" && mkdir -p "$GEN"
cargo run -q -p fstudio-ffi --features bindgen --bin uniffi-bindgen -- \
  generate --library "$OUT/libfstudio_ffi.dylib" --language swift --out-dir "$GEN" >/dev/null

# Header and module map: only replaced when they change.
HDR="$ROOT/target/uniffi-headers"
mkdir -p "$HDR"
cmp -s "$GEN/fstudio_ffiFFI.h" "$HDR/fstudio_ffiFFI.h" || cp "$GEN/fstudio_ffiFFI.h" "$HDR/"
cmp -s "$GEN/fstudio_ffiFFI.modulemap" "$HDR/module.modulemap" ||
  cp "$GEN/fstudio_ffiFFI.modulemap" "$HDR/module.modulemap"

SWIFT_OUT="$ROOT/apple/FileStudio/Core/Generated"
mkdir -p "$SWIFT_OUT"
if ! cmp -s "$GEN/fstudio_ffi.swift" "$SWIFT_OUT/fstudio_ffi.swift"; then
  cp "$GEN/fstudio_ffi.swift" "$SWIFT_OUT/fstudio_ffi.swift"
  echo "bindings updated"
fi
echo "core built ($PROFILE): $OUT/libfstudio_ffi.a"
