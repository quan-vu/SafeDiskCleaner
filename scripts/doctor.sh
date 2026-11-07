#!/bin/sh
set -eu
test "$(uname -s)" = Darwin || { echo 'macOS is required' >&2; exit 1; }
command -v go >/dev/null || { echo 'Go is missing' >&2; exit 1; }
command -v xcrun >/dev/null || { echo 'Xcode command-line tools are missing' >&2; exit 1; }
command -v claude >/dev/null || echo 'Claude CLI missing: scanning will use manual review.' >&2
VERSION=$(go version | awk '{print $3}' | sed 's/^go//')
python3 - "$VERSION" <<'PY'
import sys
parts = tuple(int(x) for x in sys.argv[1].split('.')[:3])
if parts < (1, 27, 3):
    raise SystemExit('Go 1.27.3 or newer is required (found ' + sys.argv[1] + ')')
PY
FREE_KB=$(df -Pk . | awk 'NR==2 {print $4}')
if [ "$FREE_KB" -lt 2097152 ]; then
  echo 'At least 2 GB free is required for Go toolchain and macOS build.' >&2
  exit 1
fi
echo 'macOS build prerequisites are available.'
