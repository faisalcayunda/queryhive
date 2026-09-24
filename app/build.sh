#!/bin/bash
# Builds "dist/QueryHive.app" around the Rust engine's shared library.
# Needs only the Xcode Command Line Tools.
#
# Known limitation, deliberately not solved here: the Swift binary links
# `libqh_ffi.dylib` by the absolute install name Cargo gives a `cdylib`, so the
# bundle runs on this machine (where `target/release` exists) and not on another
# one. Making the bundle relocatable means building the crate as a `staticlib`
# and packaging it as an XCFramework from an xtask -- a change to the FFI crate
# rather than to this script (app/Package.swift says the same thing at
# `ffiLibraryDirectory`).
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

# The bundle holds one Mach-O, the app binary, and its engine is whatever
# `libqh_ffi.dylib` it links. Sign that, then the bundle. Static objects/archives
# are never executed and codesign refuses to sign them anyway, so the find skips
# them; it stays a loop because a future XCFramework would add more Mach-Os.
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
