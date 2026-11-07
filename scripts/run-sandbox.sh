#!/bin/zsh
# Run the bundled app against a fake HOME (separate settings DB, separate files).
# The real app instance is quit first because both share one bundle id.
# Usage: scripts/run-sandbox.sh [sandbox_dir]   (default: dev-sandbox/home)
set -euo pipefail
cd "$(dirname "$0")/.."
HOME_DIR=${1:-$PWD/dev-sandbox/home}
APP="$PWD/dist/Safety Disk Cleaner.app"
[[ -d $HOME_DIR ]] || { echo "Run scripts/make-test-files.sh first" >&2; exit 1; }
[[ -d $APP ]] || ./scripts/bundle.sh
osascript -e 'tell application id "io.github.quan-vu.safetydiskcleaner" to quit' 2>/dev/null || true
sleep 1
HOME="$HOME_DIR" SDC_DEBUG=1 "$APP/Contents/MacOS/safety-disk-cleaner" >"$HOME_DIR/../app.log" 2>&1 &
echo "Sandbox app started (pid $!), HOME=$HOME_DIR"
