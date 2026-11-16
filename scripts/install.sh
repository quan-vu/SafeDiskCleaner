#!/bin/zsh
# Build Safety Disk Cleaner and install it as the one and only copy in /Applications.
# Usage: scripts/install.sh [--open]
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
NAME="Safety Disk Cleaner.app"
BUNDLE_ID="io.github.quan-vu.safetydiskcleaner"
DEST="/Applications/$NAME"
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

./scripts/bundle.sh
BUILT="$ROOT/dist/$NAME"

# Quit a running instance so the new version is the one that starts.
if pgrep -x safety-disk-cleaner >/dev/null; then
  osascript -e "quit app id \"$BUNDLE_ID\"" >/dev/null 2>&1 || true
  for _ in {1..50}; do pgrep -x safety-disk-cleaner >/dev/null || break; sleep 0.1; done
  pkill -x safety-disk-cleaner 2>/dev/null || true
fi

# Remove older installs.
for old in "$DEST" "$HOME/Applications/$NAME"; do
  if [[ -d $old ]]; then
    "$LSREGISTER" -u "$old" 2>/dev/null || true
    rm -rf "$old"
  fi
done

ditto "$BUILT" "$DEST"
# Drop the build copy so Spotlight, Launchpad and "Open With" list only one app.
"$LSREGISTER" -u "$BUILT" 2>/dev/null || true
rm -rf "$BUILT"
"$LSREGISTER" -f "$DEST"

VERSION=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$DEST/Contents/Info.plist")
echo "Installed $DEST ($VERSION)"

if [[ ${1:-} == --open ]]; then
  open "$DEST"
fi
