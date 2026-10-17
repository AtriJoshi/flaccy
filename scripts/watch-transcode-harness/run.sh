#!/usr/bin/env bash
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$HERE"
swift build -c release 2>&1 | tail -5
exec "$HERE/.build/release/harness"
