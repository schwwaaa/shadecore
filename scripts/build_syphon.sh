#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SY_SOURCE="$ROOT/vendor/Syphon-Framework"
XCODEPROJ="$SY_SOURCE/Syphon.xcodeproj"

OUT_BASE="$ROOT/target/syphon_build"
DERIVED="$OUT_BASE/DerivedData"

mkdir -p "$OUT_BASE"

# Pick a scheme name without hardcoding it.
# (Syphon projects usually have "Syphon" but this makes it future-proof.)
SCHEME="$(
  xcodebuild -list -project "$XCODEPROJ" \
    | awk '/Schemes:/{flag=1;next} flag && NF{print; exit}'
)"

echo "[syphon] Using scheme: $SCHEME"
echo "[syphon] Building to: $OUT_BASE"

# Build a universal macOS framework (arm64 + x86_64) into DerivedData
xcodebuild \
  -project "$XCODEPROJ" \
  -scheme "$SCHEME" \
  -configuration Release \
  -sdk macosx \
  -derivedDataPath "$DERIVED" \
  ONLY_ACTIVE_ARCH=NO \
  ARCHS="arm64 x86_64" \
  build

# Copy the built framework to a stable location
PROD="$DERIVED/Build/Products/Release/Syphon.framework"
if [ ! -d "$PROD" ]; then
  echo "[syphon] ERROR: expected framework at: $PROD"
  echo "[syphon] Try opening the xcodeproj once in Xcode to let it resolve settings, then rerun."
  exit 1
fi

rm -rf "$OUT_BASE/Syphon.framework"
cp -R "$PROD" "$OUT_BASE/Syphon.framework"

echo "[syphon] Built: $OUT_BASE/Syphon.framework"

