#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${VERSION:-1.1.3}"
export VERSION

"$ROOT/scripts/build-appimage.sh"
"$ROOT/scripts/build-windows.sh"

cd "$ROOT/dist"
sha256sum SourceTube-x86_64.AppImage SourceTube-Windows.zip > SHA256SUMS
cat SHA256SUMS
ls -lh
