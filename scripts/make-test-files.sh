#!/bin/zsh
# Create a throw-away fake HOME full of large, old *sparse* files for testing
# the cleaner without touching real data. Sparse files report >1 GB but use
# almost no disk space.
# Usage: scripts/make-test-files.sh [sandbox_dir]   (default: dev-sandbox/home)
set -euo pipefail
cd "$(dirname "$0")/.."
HOME_DIR=${1:-$PWD/dev-sandbox/home}
D="$HOME_DIR/Downloads"
rm -rf "$HOME_DIR"
mkdir -p "$D/installers" "$D/.hidden-cache" "$D/project-repo/.git" "$D/Old Photos.photoslibrary" "$HOME_DIR/Library/Caches"

old() { touch -t 202401151030 "$@"; }   # far older than the 90-day rule

# Should be suggested (large + old)
mkfile -n 1200m "$D/ubuntu-24.04-desktop-amd64.iso"
mkfile -n 2500m "$D/Xcode_15.4.xip"
mkfile -n 4g    "$D/screen-recording-2024-01.mov"
mkfile -n 1100m "$D/installers/Docker-4.27.dmg"
mkfile -n 1500m "$D/installers/backup-site-old.zip"
old "$D/ubuntu-24.04-desktop-amd64.iso" "$D/Xcode_15.4.xip" "$D/screen-recording-2024-01.mov" \
    "$D/installers/Docker-4.27.dmg" "$D/installers/backup-site-old.zip"

# Must NOT be suggested
mkfile -n 2g    "$D/recent-download.dmg"                       # modified today
mkfile -n 100m  "$D/small-old-file.pdf"; old "$D/small-old-file.pdf"   # below 500 MB
mkfile -n 1500m "$D/.hidden-cache/blob.bin"; old "$D/.hidden-cache/blob.bin"   # hidden dir
mkfile -n 1500m "$D/project-repo/build.bin"; old "$D/project-repo/build.bin"   # inside a git checkout
mkfile -n 1500m "$D/Old Photos.photoslibrary/db.bin"; old "$D/Old Photos.photoslibrary/db.bin"
mkfile -n 1500m "$HOME_DIR/Library/Caches/huge.cache"; old "$HOME_DIR/Library/Caches/huge.cache"
ln -s "$D/Xcode_15.4.xip" "$D/link-to-xcode.xip"               # symlink

echo "Sandbox HOME: $HOME_DIR"
echo "Apparent size: $(du -sh -A "$HOME_DIR" | cut -f1)   real disk use: $(du -sh "$HOME_DIR" | cut -f1)"
