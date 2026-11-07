#!/bin/zsh
# Build Safety Disk Cleaner.app (and optionally a DMG) from the Rust workspace.
# Usage: scripts/bundle.sh [--dmg] [--debug]
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
PROFILE=release
DMG=0
for arg in "$@"; do
  case $arg in
    --dmg) DMG=1 ;;
    --debug) PROFILE=debug ;;
  esac
done
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
if [[ $PROFILE == release ]]; then cargo build --release -p sdc-app; else cargo build -p sdc-app; fi

APP="$ROOT/dist/Safety Disk Cleaner.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$ROOT/target/$PROFILE/safety-disk-cleaner" "$APP/Contents/MacOS/"
sed "s/__VERSION__/$VERSION/g" bundle/Info.plist > "$APP/Contents/Info.plist"

ICON_SRC="$ROOT/../resources/icon.png"
if [[ -f $ICON_SRC ]]; then
  SET=$(mktemp -d)/AppIcon.iconset
  mkdir -p "$SET"
  for s in 16 32 128 256 512; do
    sips -z $s $s "$ICON_SRC" --out "$SET/icon_${s}x${s}.png" >/dev/null
    sips -z $((s*2)) $((s*2)) "$ICON_SRC" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$SET" -o "$APP/Contents/Resources/AppIcon.icns"
fi

codesign --force --deep --sign - "$APP"
echo "Built $APP"

if (( DMG )); then
  OUT="$ROOT/dist/safety-disk-cleaner-$VERSION-arm64.dmg"
  STAGE=$(mktemp -d)
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  rm -f "$OUT"
  hdiutil create -volname "Safety Disk Cleaner" -srcfolder "$STAGE" -ov -format UDZO "$OUT" >/dev/null
  echo "Built $OUT"
fi
