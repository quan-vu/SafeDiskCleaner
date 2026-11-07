#!/bin/sh
set -eu
SOURCE=$1
DEST=$2
IDENTIFIER=io.github.quan-vu.safetydiskcleaner
test -d "$SOURCE" || { echo "Missing build: $SOURCE" >&2; exit 1; }
bundle_id() { /usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$1/Contents/Info.plist" 2>/dev/null || true; }
test "$(bundle_id "$SOURCE")" = "$IDENTIFIER" || { echo 'Unexpected source bundle ID' >&2; exit 1; }
if [ -d "$DEST" ]; then
  test "$(bundle_id "$DEST")" = "$IDENTIFIER" || { echo 'Destination contains another app' >&2; exit 1; }
fi
LEGACY="$HOME/Applications/Safety Disk Cleaner.app"
if [ -d "$LEGACY" ]; then
  test "$(bundle_id "$LEGACY")" = "$IDENTIFIER" || { echo 'Another app uses the legacy path' >&2; exit 1; }
fi
mkdir -p /Applications
TEMP="${DEST}.installing.$$"
trap 'rm -rf "$TEMP"' EXIT INT TERM
ditto "$SOURCE" "$TEMP"
test "$(bundle_id "$TEMP")" = "$IDENTIFIER" || { echo 'Copied app failed identity check' >&2; exit 1; }
osascript -e 'tell application id "io.github.quan-vu.safetydiskcleaner" to quit' >/dev/null 2>&1 || true
if [ -d "$DEST" ]; then rm -rf "$DEST"; fi
mv "$TEMP" "$DEST"
if [ -d "$LEGACY" ]; then rm -rf "$LEGACY"; fi
echo "Installed one managed copy at $DEST"
