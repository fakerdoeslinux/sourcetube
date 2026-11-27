#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${VERSION:-1.0.0}"
NODE_VERSION="${NODE_VERSION:-v26.13.1}"
OUTPUT="${OUTPUT:-$ROOT/dist/SourceTube-x86_64.AppImage}"
CACHE="$ROOT/build/cache"
APPDIR="$ROOT/build/SourceTube.AppDir"

rm -rf "$APPDIR"
mkdir -p "$ROOT/dist" "$CACHE" "$APPDIR/usr/share/sourcetube" \
  "$APPDIR/usr/lib" "$APPDIR/usr/share/icons/hicolor/256x256/apps"

NODE_TAR="$CACHE/node-$NODE_VERSION-linux-x64.tar.xz"
if [ ! -f "$NODE_TAR" ]; then
  echo "Downloading Node $NODE_VERSION..."
  curl -fsSL "https://nodejs.org/dist/$NODE_VERSION/node-$NODE_VERSION-linux-x64.tar.xz" -o "$NODE_TAR"
fi
rm -rf "$APPDIR/usr/lib/node"
mkdir -p "$APPDIR/usr/lib/node"
tar -xJf "$NODE_TAR" -C "$APPDIR/usr/lib/node" --strip-components=1

TOOL="$CACHE/appimagetool-x86_64.AppImage"
if [ ! -f "$TOOL" ]; then
  echo "Downloading appimagetool..."
  curl -fsSL "https://github.com/AppImage/AppImageKit/releases/download/continuous/appimagetool-x86_64.AppImage" -o "$TOOL"
  chmod +x "$TOOL"
fi

cp "$ROOT/server.mjs" "$ROOT/package.json" "$APPDIR/usr/share/sourcetube/"
cp -r "$ROOT/public" "$APPDIR/usr/share/sourcetube/"
cp "$ROOT/.env.example" "$APPDIR/usr/share/sourcetube/"
cp "$ROOT/public/icon.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/sourcetube.svg" 2>/dev/null || true
mkdir -p "$APPDIR/usr/share/icons/hicolor/scalable/apps"
cp "$ROOT/public/icon.svg" "$APPDIR/usr/share/icons/hicolor/scalable/apps/sourcetube.svg"
rsvg-convert -w 256 -h 256 "$ROOT/public/icon.svg" -o "$APPDIR/usr/share/icons/hicolor/256x256/apps/sourcetube.png"
cp "$APPDIR/usr/share/icons/hicolor/256x256/apps/sourcetube.png" "$APPDIR/sourcetube.png"

cat > "$APPDIR/AppRun" <<'APPRUN_EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
export PATH="$HERE/usr/lib/node/bin:$PATH"

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

echo "SourceTube __VERSION__ running at $APP_ORIGIN"
exec "$HERE/usr/lib/node/bin/node" "$HERE/usr/share/sourcetube/server.mjs"
APPRUN_EOF
sed -i "s/__VERSION__/$VERSION/" "$APPDIR/AppRun"
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
Categories=AudioVideo;Video;
StartupNotify=true
DESKTOP
mkdir -p "$APPDIR/usr/share/applications"
cp "$APPDIR/$DESKTOP_FILE" "$APPDIR/usr/share/applications/$DESKTOP_FILE"
mkdir -p "$APPDIR/usr/share/metainfo"
cp "$ROOT/assets/sourcetube.appdata.xml" "$APPDIR/usr/share/metainfo/io.github.fakerdoeslinux.sourcetube.appdata.xml"
ln -sf "usr/share/icons/hicolor/256x256/apps/sourcetube.png" "$APPDIR/.DirIcon"

echo "Building $OUTPUT..."
ARCH=x86_64 "$TOOL" --appimage-extract-and-run "$APPDIR" "$OUTPUT"
chmod +x "$OUTPUT"
echo "Built: $OUTPUT"
