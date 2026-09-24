#!/bin/bash
# Builds "dist/QueryHive.app" around the Rust engine.
# Needs only the Xcode Command Line Tools.
#
# The bundle is self-contained. `app/build-ffi.sh` stages the engine's static archive
# (target/ffi/static/release/libqh_ffi.a) and this script's `swift build` links that
# archive rather than the `cdylib`, so the Rust code is copied into the app binary and
# the app loads no Rust library at runtime: `otool -L dist/QueryHive.app/Contents/MacOS/
# QueryHive` lists Apple frameworks and nothing from `target/`. That is what makes the
# DMG portable. The bundle is one Mach-O, and the signing loop below stays a loop so a
# second one would not go unsigned.
set -euo pipefail
cd "$(dirname "$0")"

# One source of truth for the version: the crate the app is built against.
# `engineVersion()` in the FFI returns this same string at runtime
# (`env!("CARGO_PKG_VERSION")`), so a mismatch would be visible in the UI.
VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' ../crates/qh-ffi/Cargo.toml)}"
VERSION="${VERSION:-0.1.0}"
BUILD="${BUILD:-$(date +%Y%m%d%H%M)}"

if [ ! -f ./build-ffi.sh ]; then
    echo "app/build-ffi.sh is missing"
    exit 1
fi
# Builds the release dylib the app links and regenerates app/Generated/ from it.
# It has to run first: `swift build` has no pre-build hook of its own.
./build-ffi.sh

# --disable-sandbox: SwiftPM wraps its own manifest compile in sandbox-exec, which fails when
# this script is itself run from inside another sandbox ("sandbox_apply: Operation not
# permitted"). The package has no build plugins, so nothing is lost by turning it off.
swift build -c release --disable-sandbox
app="dist/QueryHive.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$(swift build -c release --disable-sandbox --show-bin-path)/QueryHive" "$app/Contents/MacOS/"

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

# The bundle holds one Mach-O, the app binary, and the engine is inside it: `swift build`
# copied the static archive's code into that binary, so there is no second file to sign.
# The loop stays a loop because a second Mach-O would otherwise go unsigned. Static
# objects and archives are never executed and codesign refuses to sign them anyway, so
# the find skips them; a `.a` never reaches this bundle in the first place.
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
