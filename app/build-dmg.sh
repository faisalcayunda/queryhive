#!/bin/bash
# Builds app/dist/QueryHive-<version>-arm64.dmg
#
#   ./app/build-dmg.sh                  ad-hoc signed (the default; see "Gatekeeper" below)
#   ./app/build-dmg.sh --developer-id   re-sign with a Developer ID + hardened runtime
#   ./app/build-dmg.sh --notarize       also submit to Apple and staple the ticket
#
# Gatekeeper
# ----------
# An ad-hoc signature is internally valid -- `codesign --verify --deep --strict` passes, and the
# app does not report as damaged -- but Apple has not vetted it, so a *downloaded* copy is
# quarantined and macOS refuses the first launch until the user allows it by hand. There is no
# way around that except notarization, and notarization needs a paid Apple Developer Program
# membership plus a "Developer ID Application" certificate. Measured on this machine: an "Apple
# Development" certificate is rejected by `spctl` exactly like ad-hoc (`origin=Apple
# Development: ...`), so it buys nothing for distribution and is not used here.
#
# The DMG therefore carries a READ ME with the one-time unlock, and --developer-id/--notarize
# are here so that acquiring the certificate is the only missing piece.
set -euo pipefail
cd "$(dirname "$0")"

APP="QueryHive"
# The crate the app is built against, which is also what `engineVersion()` reports.
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' ../crates/qh-ffi/Cargo.toml)"
VERSION="${VERSION:-0.0.1}"
ARCH="$(uname -m)"
DEVELOPER_ID=false
NOTARIZE=false
for arg in "$@"; do
  case "$arg" in
    --developer-id) DEVELOPER_ID=true ;;
    --notarize) DEVELOPER_ID=true; NOTARIZE=true ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

if [[ "$ARCH" != "arm64" ]]; then
  echo "This build targets Apple Silicon; you are on $ARCH." >&2
  exit 1
fi

echo "==> app ($ARCH)"
./build.sh

BUNDLE="dist/$APP.app"

# ---------------------------------------------------------------- signing
if $DEVELOPER_ID; then
  IDENTITY="${IDENTITY:-$(security find-identity -v -p codesigning \
    | sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -1)}"
  if [[ -z "$IDENTITY" ]]; then
    cat >&2 <<'MISSING'
No "Developer ID Application" certificate found.

To distribute without the first-launch prompt you need:
  1. A paid Apple Developer Program membership.
  2. A "Developer ID Application" certificate in your keychain
     (Xcode > Settings > Accounts > Manage Certificates > + > Developer ID Application).
  3. Notarization credentials stored once:
       xcrun notarytool store-credentials queryhive-notary \
         --apple-id <you@example.com> --team-id <TEAMID> --password <app-specific-password>

Then:  IDENTITY="Developer ID Application: ..." NOTARY_PROFILE=queryhive-notary ./app/build-dmg.sh --notarize

An "Apple Development" certificate does not work here -- Gatekeeper rejects it for distribution.
MISSING
    exit 1
  fi
  echo "    signing as: $IDENTITY"
  # Every Mach-O inside the bundle, then the bundle. Today that is the app binary alone -- the
  # engine is static, so its code is inside that binary rather than beside it; the loop is here
  # so that a second Mach-O would not silently go unsigned.
  find "$BUNDLE" -type f -print0 | while IFS= read -r -d '' f; do
    case "$f" in *.o|*.a) continue ;; esac
    if file -b "$f" | grep -q "Mach-O"; then
      codesign --force --options runtime --timestamp \
        --entitlements QueryHive.entitlements --sign "$IDENTITY" "$f"
    fi
  done
  codesign --force --options runtime --timestamp \
    --entitlements QueryHive.entitlements --sign "$IDENTITY" "$BUNDLE"
else
  echo "    ad-hoc signature (Gatekeeper will require a one-time unlock on other Macs)"
fi
codesign --verify --deep --strict "$BUNDLE"

# ---------------------------------------------------------------- notarize the app
if $NOTARIZE; then
  PROFILE="${NOTARY_PROFILE:-queryhive-notary}"
  echo "==> notarizing the app (profile: $PROFILE)"
  ZIP="dist/$APP-notarize.zip"
  rm -f "$ZIP"
  ditto -c -k --keepParent "$BUNDLE" "$ZIP"
  xcrun notarytool submit "$ZIP" --keychain-profile "$PROFILE" --wait
  rm -f "$ZIP"
  xcrun stapler staple "$BUNDLE"
  xcrun stapler validate "$BUNDLE"
fi

# ---------------------------------------------------------------- DMG
echo "==> dmg"
DMG="dist/$APP-$VERSION-arm64.dmg"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$BUNDLE" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
cat > "$STAGE/READ ME.txt" <<'README'
QueryHive
=========

1.  Drag QueryHive to the Applications folder.

2.  FIRST LAUNCH — this build is signed but not notarized by Apple, so macOS blocks the
    very first launch. Any one of these unlocks it, once:

      macOS 15 (Sequoia) and later
        System Settings > Privacy & Security, scroll to Security, click "Open Anyway"
        next to QueryHive.

      macOS 14 (Sonoma)
        Right-click QueryHive in Applications, choose Open, then Open again.

      Any version, from Terminal
        xattr -dr com.apple.quarantine /Applications/QueryHive.app

3.  Requires an Apple Silicon Mac (M1 or later) and macOS 14 or later.

4.  Nothing else to install. The engine is native Rust. No Python, no pip, no
    virtualenv, no PATH setup.
README

rm -f "$DMG"
hdiutil create -volname "$APP" -srcfolder "$STAGE" -ov -format UDZO -quiet "$DMG"

if $DEVELOPER_ID; then
  codesign --force --timestamp --sign "$IDENTITY" "$DMG"
fi
if $NOTARIZE; then
  xcrun notarytool submit "$DMG" --keychain-profile "$NOTARY_PROFILE" --wait 2>/dev/null \
    || xcrun notarytool submit "$DMG" --keychain-profile queryhive-notary --wait
  xcrun stapler staple "$DMG"
fi

# ---------------------------------------------------------------- verify the artifact
echo "==> verifying the dmg"
MOUNT="$(mktemp -d)"
hdiutil attach "$DMG" -mountpoint "$MOUNT" -nobrowse -quiet
trap 'hdiutil detach "$MOUNT" -quiet || true; rm -rf "$STAGE" "$MOUNT"' EXIT

codesign --verify --deep --strict "$MOUNT/$APP.app"
echo "    signature ok"

BIN="$MOUNT/$APP.app/Contents/MacOS/$APP"
[[ "$(file -b "$BIN")" == *arm64* ]] || { echo "    not arm64" >&2; exit 1; }
echo "    arch ok (arm64)"

MINOS="$(otool -l "$BIN" | awk '/LC_BUILD_VERSION/{f=1} f&&/minos/{print $2; exit}')"
echo "    minimum macOS $MINOS"

# It has to work from the read-only volume, not only after being copied out. Checked by
# artifact, not by exit code: `--snapshot` renders the whole UI, so a PNG is proof that the
# Swift binary, its resources and the window stack all came up.
CHECK="$(mktemp -d)"
"$BIN" --snapshot "$CHECK/proof.png" --scene done
[[ -s "$CHECK/proof.png" ]] || { echo "    FAILED to render from the mounted dmg" >&2; exit 1; }
echo "    runs from the mounted dmg (rendered $(du -h "$CHECK/proof.png" | cut -f1) of UI)"
rm -rf "$CHECK"

if $NOTARIZE; then
  spctl -a -t exec -vv "$MOUNT/$APP.app" && echo "    Gatekeeper accepts it"
else
  echo "    Gatekeeper: rejected until the one-time unlock in READ ME.txt (expected for ad-hoc)"
fi

echo
echo "built: $DMG  ($(du -h "$DMG" | cut -f1))"
echo "sha256: $(shasum -a 256 "$DMG" | cut -d' ' -f1)"
