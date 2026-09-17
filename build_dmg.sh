#!/usr/bin/env bash
# Build "QueryHive.app" and wrap it in a DMG for Apple Silicon.
#
#   ./build_dmg.sh            -> dist/QueryHive-<version>-arm64.dmg
#
# The .app is unsigned: on first launch macOS Gatekeeper will complain.
# Right-click > Open, or run:  xattr -dr com.apple.quarantine "/Applications/QueryHive.app"
set -euo pipefail
cd "$(dirname "$0")"

APP="QueryHive"
BUNDLE_ID="id.data-ecosystem.queryhive"
VERSION="$(sed -n 's/^__version__ = "\(.*\)"/\1/p' exporter/__init__.py)"
ARCH="$(uname -m)"

if [[ "$ARCH" != "arm64" ]]; then
  echo "This script targets Apple Silicon; you are on $ARCH." >&2
  echo "PyInstaller cannot cross-compile: build on an arm64 Mac." >&2
  exit 1
fi

echo "==> venv"
source ./venv_setup.sh
ensure_venv
VIRTUAL_ENV=.venv uv pip install -q pyinstaller

echo "==> checks (on $(.venv/bin/python -c 'import sys,sysconfig;print(sys.version.split()[0], "free-threaded" if sysconfig.get_config_var("Py_GIL_DISABLED") else "GIL")'))"
for t in tests/test_writers.py tests/test_retry.py tests/test_prefetch.py; do
  echo "    $t"
  .venv/bin/python "$t" >/dev/null
done

echo "==> pyinstaller ($ARCH)"
rm -rf build dist
.venv/bin/pyinstaller \
  --noconfirm --clean --windowed \
  --name "$APP" \
  --osx-bundle-identifier "$BUNDLE_ID" \
  --icon assets/icon.icns \
  --target-arch arm64 \
  --add-data "exporter/static:exporter/static" \
  --collect-submodules uvicorn \
  --collect-submodules trino \
  --collect-submodules webview \
  --hidden-import uvicorn.logging \
  --hidden-import uvicorn.loops.auto \
  --hidden-import uvicorn.protocols.http.auto \
  --hidden-import uvicorn.protocols.websockets.auto \
  --hidden-import uvicorn.lifespan.on \
  app.py

# Info.plist tweaks: real version, and no Dock bouncing for a browser-driven app
PLIST="dist/$APP.app/Contents/Info.plist"
plist_set() {  # Set if the key exists, Add if it does not
  /usr/libexec/PlistBuddy -c "Set :$1 $3" "$PLIST" 2>/dev/null ||
  /usr/libexec/PlistBuddy -c "Add :$1 $2 $3" "$PLIST" >/dev/null
}
plist_set CFBundleShortVersionString string "$VERSION"
plist_set CFBundleVersion string "$VERSION"
plist_set NSHighResolutionCapable bool true

echo "==> re-signing the bundle (required after Info.plist tweaks on arm64)"
codesign --force --deep -s - "dist/$APP.app"

echo "==> smoke test the bundle"
QUERYHIVE_PORT=18999 "dist/$APP.app/Contents/MacOS/$APP" --no-browser &
SMOKE_PID=$!
trap 'kill $SMOKE_PID 2>/dev/null || true' EXIT
for _ in $(seq 1 40); do
  curl -sf http://127.0.0.1:18999/api/formats >/dev/null && break
  sleep 0.5
done
curl -sf http://127.0.0.1:18999/api/formats >/dev/null || { echo "bundle failed to serve" >&2; exit 1; }
kill $SMOKE_PID 2>/dev/null || true
trap - EXIT
echo "    bundle serves ok"

echo "==> dmg"
STAGE="$(mktemp -d)"
cp -R "dist/$APP.app" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
DMG="dist/QueryHive-$VERSION-arm64.dmg"
rm -f "$DMG"
hdiutil create -volname "$APP" -srcfolder "$STAGE" -ov -format UDZO -quiet "$DMG"
rm -rf "$STAGE"

echo
echo "built: $DMG  ($(du -h "$DMG" | cut -f1))"
