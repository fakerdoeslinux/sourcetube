#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${VERSION:-1.1.3}"
STAGE="$ROOT/build/SourceTube-Windows"
ZIP="$ROOT/dist/SourceTube-Windows.zip"

echo "Building Windows release binaries..."
if [ "$(uname -s)" = "Linux" ] && command -v cargo-xwin >/dev/null 2>&1; then
  (cd "$ROOT" && cargo xwin build --release --target x86_64-pc-windows-msvc)
  BIN_DIR="$ROOT/target/x86_64-pc-windows-msvc/release"
elif [ "$(uname -s)" = "Linux" ]; then
  (cd "$ROOT" && cargo build --release --target x86_64-pc-windows-gnu)
  BIN_DIR="$ROOT/target/x86_64-pc-windows-gnu/release"
else
  (cd "$ROOT" && cargo build --release)
  BIN_DIR="$ROOT/target/release"
fi

if [ ! -f "$BIN_DIR/sourcetube.exe" ] || [ ! -f "$BIN_DIR/sourcetube-player.exe" ]; then
  echo "Windows binaries missing in $BIN_DIR" >&2
  exit 1
fi

rm -rf "$STAGE"
mkdir -p "$STAGE/app/public" "$ROOT/dist"

cp "$BIN_DIR/sourcetube.exe" "$STAGE/app/sourcetube.exe"
cp "$BIN_DIR/sourcetube-player.exe" "$STAGE/app/sourcetube-player.exe"
cp "$ROOT/.env.example" "$STAGE/app/.env.example"
cp -R "$ROOT/public/"* "$STAGE/app/public/"
cp "$ROOT/assets/adblock-rules.txt" "$STAGE/app/adblock-rules.txt"
cp "$ROOT/assets/sourcetube.ico" "$STAGE/app/sourcetube.ico"
cp "$ROOT/assets/sourcetube-512.png" "$STAGE/app/sourcetube-512.png"
cp "$ROOT/windows/sourcetube.cmd" "$ROOT/windows/START-HERE.txt" "$STAGE/"

cat > "$STAGE/README.txt" <<EOF
SourceTube $VERSION for Windows

1. Extract the ZIP somewhere writable, for example Documents\\SourceTube.
2. Copy app\\.env.example to app\\.env and fill in your credentials.
3. Double-click sourcetube.cmd.
4. The browser opens automatically on the selected port. If 8787 was busy, a
   different port is used and the browser opens there instead.

Browser playback uses the bundled DASH player and the local SourceTube proxy;
no third-party player is loaded. Native playback uses the bundled Rust player
through mpv/libmpv and accepts progressive, DASH and HLS URLs. Install mpv for
Windows and keep mpv.exe in PATH for low-RAM playback.

Version 1.1.3 includes SponsorBlock, bundled adblock rules, the transfer wizard
and the new icon. All fonts, rules and player libraries are local.

No native macOS build. Use the web client in the main README.
EOF

python3 -m zipfile -c "$ZIP" "$STAGE"
echo "Built: $ZIP"
