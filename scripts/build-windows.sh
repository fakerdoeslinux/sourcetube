#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${VERSION:-1.0.0}"
STAGE="$ROOT/build/SourceTube-Windows"
ZIP="$ROOT/dist/SourceTube-Windows.zip"

rm -rf "$STAGE"
mkdir -p "$STAGE/app" "$ROOT/dist"

cp "$ROOT/server.mjs" "$ROOT/package.json" "$ROOT/.env.example" "$STAGE/app/"
cp -r "$ROOT/public" "$STAGE/app/"
cp "$ROOT/windows/sourcetube.cmd" "$ROOT/windows/START-HERE.txt" "$STAGE/"

cat > "$STAGE/README.txt" <<EOF
SourceTube $VERSION for Windows

1. Install Node.js 20 or newer from https://nodejs.org
2. Copy .env.example in the app folder to .env and fill in credentials
3. Double-click sourcetube.cmd
4. Open http://localhost:8787

No native macOS build. Use the web client in the main README.
EOF

python3 -m zipfile -c "$ZIP" "$STAGE"
echo "Built: $ZIP"
