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

# Where the app looks for releases, and the key every update is checked against. Both are
# overridable so a release candidate can be pointed at a folder on this machine instead of at
# GitHub — that is the only way to test an update without publishing one.
#
# `appcast.xml` is the one asset name that matters: GitHub's `releases/latest/download/<asset>`
# redirects to the newest release's copy of it, so this URL never has to change.
FEED_URL="${FEED_URL:-https://github.com/faisalcayunda/queryhive/releases/latest/download/appcast.xml}"
PUBLIC_KEY="${PUBLIC_KEY:-$(cat sparkle-public-key.txt 2>/dev/null || true)}"
if [ -z "$PUBLIC_KEY" ]; then
    echo "app/sparkle-public-key.txt is missing — run Sparkle's generate_keys and save its public half there"
    exit 1
fi

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

# The MCP server, beside the app binary. A separate process on purpose (Fase 2): it
# shares nothing with the app but the database file and the Keychain, so a client that
# kills it cannot touch the app, and the app never spawns it — the user's MCP client
# does, pointing at this path. `build-ffi.sh` above already built it, because that is a
# plain `cargo build -p qh-ffi --release` and the binary is one of the package's targets.
MCP_BIN="../target/release/queryhive-mcp"
if [ ! -x "$MCP_BIN" ]; then
    echo "queryhive-mcp was not built — app/build-ffi.sh should have produced $MCP_BIN"
    exit 1
fi
cp "$MCP_BIN" "$app/Contents/MacOS/queryhive-mcp"

# Sparkle is the bundle's one dynamic framework, and the only thing here that did not come from
# this repository. `ditto` rather than `cp -R`: a framework is mostly symlinks (`Sparkle`,
# `Versions/Current`, `Resources`), and copying them as files would leave a layout that codesign
# and dyld both refuse. SwiftPM has already unpacked the framework beside the build product, which
# is where this reads it from — the same copy the debug build and the test bundle load.
SPARKLE_FRAMEWORK="$(swift build -c release --disable-sandbox --show-bin-path)/Sparkle.framework"
if [ ! -d "$SPARKLE_FRAMEWORK" ]; then
    echo "Sparkle.framework is not among the build products — run 'swift package resolve' first"
    exit 1
fi
mkdir -p "$app/Contents/Frameworks"
ditto "$SPARKLE_FRAMEWORK" "$app/Contents/Frameworks/Sparkle.framework"

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
  <key>NSHumanReadableCopyright</key><string>Copyright © 2026 Faisal Nugraha Cayunda</string>
</dict>
</plist>
EOF

# Set after the heredoc so VERSION/BUILD are escaped correctly by plutil rather than
# interpolated into the plist as raw shell text.
plutil -replace CFBundleShortVersionString -string "$VERSION" "$app/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$BUILD" "$app/Contents/Info.plist"

# Sparkle's two, and the only two it needs to find and trust an update. Note which number decides
# "newer": Sparkle compares `CFBundleVersion`, not the version people read. That is why `BUILD`
# above is a timestamp and not a marketing version — two releases of 0.1.0 are still ordered, and
# a rebuild in the same minute is not (it would look like no update at all).
plutil -replace SUFeedURL -string "$FEED_URL" "$app/Contents/Info.plist"
plutil -replace SUPublicEDKey -string "$PUBLIC_KEY" "$app/Contents/Info.plist"

if [ -f ../assets/icon.icns ]; then
    cp ../assets/icon.icns "$app/Contents/Resources/AppIcon.icns"
fi

# Two Mach-Os live here now, the app binary and Sparkle's framework; the engine is still *inside*
# the app binary, because `swift build` copied the static archive's code into it, so nothing from
# `target/` is loaded at runtime. The loop stays a loop because a third Mach-O would otherwise go
# unsigned. Static objects and archives are never executed and codesign refuses to sign them
# anyway, so the find skips them; a `.a` never reaches this bundle in the first place.
find "$app" -type f -print0 | while IFS= read -r -d '' f; do
    case "$f" in
        *.o|*.a) continue ;;
        # Sparkle's own helpers keep the signatures Sparkle shipped them with, and they are
        # already ad-hoc — the same as this bundle — so there is nothing to make them match.
        # Re-signing them here would take away what Sparkle's distribution keeps: the Downloader
        # XPC service carries an entitlement this loop does not preserve, and a nested `.app` or
        # `.xpc` has to be signed as a bundle from the inside out rather than file by file.
        # Sparkle's documentation says both of those, and the app is not sandboxed, so its
        # services are not on the update path anyway.
        "$app/Contents/Frameworks/"*) continue ;;
    esac
    if file -b "$f" | grep -q "Mach-O"; then
        codesign --force --sign - "$f"
    fi
done
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
echo "Built $app"
