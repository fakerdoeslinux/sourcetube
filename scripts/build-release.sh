#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
"$ROOT/scripts/build-appimage.sh"
"$ROOT/scripts/build-windows.sh"
ls -lh "$ROOT/dist"
