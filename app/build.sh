#!/bin/bash
# Builds "dist/QueryHive.app" with the bundled Python engine.
# Needs only the Xcode Command Line Tools.
set -euo pipefail
cd "$(dirname "$0")"

VERSION="${VERSION:-$(sed -n 's/^__version__ = "\(.*\)"/\1/p' ../exporter/__init__.py)}"
VERSION="${VERSION:-0.0.1}"
BUILD="${BUILD:-$(date +%Y%m%d%H%M)}"

if [ ! -f ./build-engine.sh ]; then
    echo "app/build-engine.sh is missing"
    exit 1
fi
./build-engine.sh

# --disable-sandbox: SwiftPM wraps its own manifest compile in sandbox-exec, which fails when
# this script is itself run from inside another sandbox ("sandbox_apply: Operation not
# permitted"). The package has no build plugins, so nothing is lost by turning it off.
swift build -c release --disable-sandbox
app="dist/QueryHive.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/engine"
cp "$(swift build -c release --disable-sandbox --show-bin-path)/QueryHive" "$app/Contents/MacOS/"

# Bundled engine: the standalone interpreter and pinned packages, built by build-engine.sh.
cp -R .engine/python "$app/Contents/Resources/engine/python"
cp -R .engine/site-packages "$app/Contents/Resources/engine/site-packages"
# Console-script shims (their shebangs point at this build machine's absolute path, and the
# app never runs them) and the empty pip lock file: neither belongs in a shipped bundle.
rm -rf "$app/Contents/Resources/engine/site-packages/bin"
rm -f "$app/Contents/Resources/engine/site-packages/.lock"
cp engine/queryhive_engine.py "$app/Contents/Resources/engine/queryhive_engine.py"

# The engine imports `exporter` from beside itself, so the package ships inside the bundle.
cp -R ../exporter "$app/Contents/Resources/engine/exporter"
# The engine only ever imports exporter.export / .source / .writers. web.py needs FastAPI,
# pydantic and keyring, cli.py needs keyring, and static/ is the 83 KB browser UI: none of
# them are in the pinned engine, and shipping an unimportable module is worse than not
# shipping it.
rm -rf "$app/Contents/Resources/engine/exporter/static"
rm -f "$app/Contents/Resources/engine/exporter/web.py"
rm -f "$app/Contents/Resources/engine/exporter/cli.py"
find "$app/Contents/Resources/engine/exporter" -type d -name "__pycache__" -exec rm -rf {} +

cat > "$app/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>QueryHive</string>
  <key>CFBundleIdentifier</key><string>id.data-ecosystem.queryhive</string>
  <key>CFBundleName</key><string>QueryHive</string>
  <key>CFBundleDisplayName</key><string>QueryHive</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF

# Set after the heredoc so VERSION/BUILD are escaped correctly by plutil rather than
# interpolated into the plist as raw shell text.
plutil -replace CFBundleShortVersionString -string "$VERSION" "$app/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$BUILD" "$app/Contents/Info.plist"

if [ -f ../assets/icon.icns ]; then
    cp ../assets/icon.icns "$app/Contents/Resources/AppIcon.icns"
fi

# Ad-hoc sign every Mach-O inside the bundle (the bundled Python interpreter and any
# compiled extensions in site-packages), then the app itself. Static objects/archives are
# never executed and codesign refuses to sign them anyway, so skip them outright.
find "$app" -type f -print0 | while IFS= read -r -d '' f; do
    case "$f" in
        *.o|*.a) continue ;;
    esac
    if file -b "$f" | grep -q "Mach-O"; then
        codesign --force --sign - "$f"
    fi
done
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
echo "Built $app"
