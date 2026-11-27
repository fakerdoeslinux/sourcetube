#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${VERSION:-1.1.1}"
OUTPUT="${OUTPUT:-$ROOT/dist/SourceTube-x86_64.AppImage}"
CACHE="$ROOT/build/cache"
APPDIR="$ROOT/build/SourceTube.AppDir"

echo "Building Rust release binaries..."
(cd "$ROOT" && cargo build --release)

rm -rf "$APPDIR"
mkdir -p "$ROOT/dist" "$CACHE" \
  "$APPDIR/usr/bin" \
  "$APPDIR/usr/share/sourcetube/public" \
  "$APPDIR/usr/share/icons/hicolor/256x256/apps" \
  "$APPDIR/usr/share/icons/hicolor/scalable/apps" \
  "$APPDIR/usr/share/applications" \
  "$APPDIR/usr/share/metainfo"

cp "$ROOT/target/release/sourcetube" "$APPDIR/usr/bin/sourcetube"
cp "$ROOT/target/release/sourcetube-player" "$APPDIR/usr/bin/sourcetube-player"
cp "$ROOT/public/"* "$APPDIR/usr/share/sourcetube/public/"
cp "$ROOT/.env.example" "$APPDIR/usr/share/sourcetube/.env.example"
cp "$ROOT/public/icon.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/sourcetube.svg"
rsvg-convert -w 256 -h 256 "$ROOT/public/icon.svg" -o "$APPDIR/usr/share/icons/hicolor/256x256/apps/sourcetube.png"
cp "$APPDIR/usr/share/icons/hicolor/256x256/apps/sourcetube.png" "$APPDIR/sourcetube.png"

TOOL="$CACHE/appimagetool-x86_64.AppImage"
if [ ! -f "$TOOL" ]; then
  echo "Downloading appimagetool..."
  curl -fsSL "https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage" -o "$TOOL"
  chmod +x "$TOOL"
fi

cat > "$APPDIR/AppRun" <<'APPRUN_EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export SOURCETUBE_PUBLIC="$HERE/usr/share/sourcetube/public"
export SOURCETUBE_PLAYER="$HERE/usr/bin/sourcetube-player"

for env_file in \
  "${XDG_CONFIG_HOME:-$HOME/.config}/sourcetube/.env" \
  "$HOME/.config/sourcetube/.env" \
  "${APPIMAGE%/*}/.env" \
  "$HERE/usr/share/sourcetube/.env"
do
  if [ -f "$env_file" ]; then
    set -a
    . "$env_file"
    set +a
  fi
done

export PORT="${PORT:-8787}"
export APP_ORIGIN="${APP_ORIGIN:-http://localhost:$PORT}"
export GOOGLE_REDIRECT_URI="${GOOGLE_REDIRECT_URI:-$APP_ORIGIN/api/auth/callback}"
export SOURCETUBE_DATA="${SOURCETUBE_DATA:-${XDG_DATA_HOME:-$HOME/.local/share}/sourcetube}"
mkdir -p "$SOURCETUBE_DATA"

if [ -z "${XDG_RUNTIME_DIR:-}" ] || [ ! -d "$XDG_RUNTIME_DIR" ] || [ ! -w "$XDG_RUNTIME_DIR" ]; then
  export XDG_RUNTIME_DIR="$SOURCETUBE_DATA"
  mkdir -p "$XDG_RUNTIME_DIR"
fi
RUNTIME_DIR="$XDG_RUNTIME_DIR"
PORT_FILE="$RUNTIME_DIR/sourcetube-port"
rm -f "$PORT_FILE"

"$HERE/usr/bin/sourcetube" > "$SOURCETUBE_DATA/server.log" 2>&1 &
SERVER_PID=$!
cleanup() {
  kill "$SERVER_PID" 2>/dev/null || true
}
trap cleanup INT TERM EXIT

FOUND_PORT=""
for _attempt in $(seq 1 50); do
  if [ -f "$PORT_FILE" ]; then
    FOUND_PORT="$(cat "$PORT_FILE" 2>/dev/null)"
    break
  fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "SourceTube server exited unexpectedly. See $SOURCETUBE_DATA/server.log" >&2
    break
  fi
  sleep 0.1
done

if [ -n "$FOUND_PORT" ] && [ -z "${SOURCETUBE_NO_BROWSER:-}" ]; then
  case "$APP_ORIGIN" in
    http://localhost:*|http://127.0.0.1:*|http://0.0.0.0:*)
      OPEN_URL="http://localhost:$FOUND_PORT"
      ;;
    *)
      OPEN_URL="$APP_ORIGIN"
      ;;
  esac
  if command -v xdg-open >/dev/null 2>&1; then
    xdg-open "$OPEN_URL" >/dev/null 2>&1 &
  elif command -v gio >/dev/null 2>&1; then
    gio open "$OPEN_URL" >/dev/null 2>&1 &
  elif command -v sensible-browser >/dev/null 2>&1; then
    sensible-browser "$OPEN_URL" >/dev/null 2>&1 &
  fi
fi

wait "$SERVER_PID"
APPRUN_EOF
chmod +x "$APPDIR/AppRun"

DESKTOP_FILE="io.github.fakerdoeslinux.sourcetube.desktop"
cat > "$APPDIR/$DESKTOP_FILE" <<DESKTOP
[Desktop Entry]
Type=Application
Name=SourceTube
GenericName=YouTube Client
Comment=Privacy-first open-source YouTube client
Exec=AppRun
Icon=sourcetube
Terminal=false
Categories=AudioVideo;Video;Network;
StartupNotify=true
DESKTOP
cp "$APPDIR/$DESKTOP_FILE" "$APPDIR/usr/share/applications/$DESKTOP_FILE"
cp "$ROOT/assets/sourcetube.appdata.xml" "$APPDIR/usr/share/metainfo/io.github.fakerdoeslinux.sourcetube.appdata.xml"
ln -sfn "usr/share/icons/hicolor/256x256/apps/sourcetube.png" "$APPDIR/.DirIcon"

echo "Building $OUTPUT..."
if ! ARCH=x86_64 "$TOOL" --appimage-extract-and-run "$APPDIR" "$OUTPUT"; then
  echo "AppStream validation unavailable; retrying with --no-appstream."
  rm -f "$OUTPUT"
  ARCH=x86_64 "$TOOL" --appimage-extract-and-run --no-appstream "$APPDIR" "$OUTPUT"
fi
chmod +x "$OUTPUT"
echo "Built: $OUTPUT"
