#!/bin/bash
# Cuts a release: package it, tag it, publish it, then check the feed the app actually reads.
#
#   ./release.sh                package, show what would be published, ask, publish
#   ./release.sh --dry-run      package only; print the publishing steps and stop
#   ./release.sh --yes          publish without the prompt (for an unattended run)
#   ./release.sh --notarize     pass through to build-dmg.sh (needs a Developer ID certificate)
#   ./release.sh --rollback <tag>   mark an older release as `latest` again (see below)
#
# What it does, in order:
#
#   1. ./build-dmg.sh          the bundle, the DMG, and its own verification — the DMG is mounted
#                              and the app is made to render from the read-only volume before it
#                              counts as built.
#   2. generate_appcast        signs the DMG with the EdDSA key in this machine's keychain and
#                              writes appcast.xml, the feed the app reads.
#   3. git tag v<version>      the tag and the bundle it describes are made from one commit.
#   4. push, gh release        main and the tag go up, then a release with **two** assets.
#
# The appcast is not optional, and it is why this is a script rather than two commands in
# hindsight: the feed URL baked into the app is
# `releases/latest/download/appcast.xml`, so the appcast has to be an asset of the release GitHub
# calls *latest*. Publish the DMG alone and you have published an app that nobody can update, with
# nothing on screen to say so.
#
# The version comes from `crates/qh-ffi/Cargo.toml`, the same crate `build.sh` and `build-dmg.sh`
# read it from, so there is one answer to "which version is this". Sparkle compares *build*
# numbers, not versions: a second release of 0.1.0 with a later timestamp is still an update, and
# two releases in the same minute are not.
#
# Notes come from `docs/releases/v<version>.md` when that file exists, so what a release says about
# itself is committed beside the code it describes rather than typed into a prompt. `RELEASE_NOTES`
# overrides it, and with neither there are `gh`'s generated notes.
#
# --rollback <tag> is the emergency exit, and it publishes, so it has no default: the tag must be
# given, must be a published vX.Y.Z release older than the current `latest` and carrying both
# assets, and its appcast must be readable. It runs `gh release edit <tag> --latest`, which moves
# the feed the app reads to that release. It does not downgrade anybody: Sparkle only offers a
# build newer than the installed one, so clients that already took the bad build see nothing. To
# reach them, release a fix with a higher build. Typed-tag confirmation unless --yes; --dry-run
# validates and prints the command. v0.0.1 has no appcast, so today there is no valid target.
# tools/release/test_release_guards.sh runs all of this against stubbed gh, git and curl.
set -euo pipefail
cd "$(dirname "$0")"

DRY_RUN=false
ASSUME_YES=false
ROLLBACK=false
ROLLBACK_TAG=""
DMG_ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=true ;;
    --yes|-y) ASSUME_YES=true ;;
    --notarize|--developer-id) DMG_ARGS+=("$1") ;;
    --rollback)
      ROLLBACK=true
      # A rollback has no default target: guessing one here would publish the guess.
      if [ $# -lt 2 ] || [[ "$2" == -* ]]; then
        echo "--rollback needs an explicit tag, for example: --rollback v0.1.0" >&2
        exit 2
      fi
      ROLLBACK_TAG="$2"
      shift
      ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
if $ROLLBACK && [ "${#DMG_ARGS[@]}" -gt 0 ]; then
  echo "--rollback publishes nothing new; ${DMG_ARGS[*]} does not apply" >&2
  exit 2
fi

# Overridable so tests can run on a machine (or a CI runner) without macOS tooling.
PLISTBUDDY="${PLISTBUDDY:-/usr/libexec/PlistBuddy}"

# Owner and repo come from the remote, so the feed URL and the download prefix cannot drift from
# where the release actually goes.
origin_slug() {
  local remote slug
  remote="$(git remote get-url origin)"
  slug="$(printf '%s' "$remote" | sed -n 's#.*github\.com[/:]##; s#\.git$##; p')"
  if [ -z "$slug" ]; then
    echo "cannot read owner/repo from origin ($remote)" >&2
    exit 1
  fi
  printf '%s' "$slug"
}

served_build() {
  curl -fsSL "$1" 2>/dev/null \
    | sed -n 's#.*<sparkle:version>\(.*\)</sparkle:version>.*#\1#p' | sed -n 1p || true
}

# Asking once is not enough, see the verify section. 0 once $1 serves build $2.
wait_for_feed() {
  local served="" attempt
  for attempt in 1 2 3 4 5 6 7 8 9 10; do
    served="$(served_build "$1")"
    if [ "$served" = "$2" ]; then return 0; fi
    echo "    feed attempt $attempt/10: served '${served:-nothing}', waiting for $2"
    sleep 6
  done
  return 1
}

# ---------------------------------------------------------------- rollback
if $ROLLBACK; then
  TAG="$ROLLBACK_TAG"
  if ! [[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "rollback target '$TAG' is not a stable vX.Y.Z tag" >&2
    exit 2
  fi
  SLUG="$(origin_slug)"
  command -v gh >/dev/null || { echo "gh is not installed" >&2; exit 1; }

  CURRENT="$(gh release view --repo "$SLUG" --json tagName --jq .tagName)"
  [ -n "$CURRENT" ] || { echo "cannot read the current latest release of $SLUG" >&2; exit 1; }
  ROW="$(gh release view "$TAG" --repo "$SLUG" --json isDraft,isPrerelease,publishedAt,assets \
    --jq '[.isDraft,.isPrerelease,(.publishedAt // "none"),([.assets[].name]|join(","))]|@tsv')" \
    || { echo "$TAG is not a release of $SLUG" >&2; exit 1; }
  IFS=$'\t' read -r IS_DRAFT IS_PRE PUBLISHED ASSETS <<<"$ROW"

  if [ "$IS_DRAFT" != false ] || [ "$IS_PRE" != false ] || [ "$PUBLISHED" = none ]; then
    echo "$TAG must be a published stable release (draft=$IS_DRAFT prerelease=$IS_PRE)" >&2
    exit 1
  fi
  if [ "$TAG" = "$CURRENT" ] || [ "$(printf '%s\n%s\n' "$TAG" "$CURRENT" | sort -V | sed -n 1p)" != "$TAG" ]; then
    echo "$TAG is not older than the current latest release $CURRENT" >&2
    exit 1
  fi
  # No appcast, no feed: moving `latest` to a release without one would break updates for everyone.
  for asset in appcast.xml "QueryHive-${TAG#v}-arm64.dmg"; do
    case ",$ASSETS," in
      *",$asset,"*) ;;
      *) echo "$TAG has no $asset asset (has: ${ASSETS:-none}); it cannot serve as latest" >&2; exit 1 ;;
    esac
  done
  WANT="$(served_build "https://github.com/$SLUG/releases/download/$TAG/appcast.xml")"
  case "$WANT" in
    ''|*[!0-9]*) echo "the appcast of $TAG does not carry a numeric build number ('${WANT:-nothing}')" >&2; exit 1 ;;
  esac

  echo "current latest   $CURRENT"
  echo "rollback target  $TAG (build $WANT)"
  echo "this moves the update feed to $TAG; clients already on a newer build are not downgraded"
  echo "would run: gh release edit $TAG --repo $SLUG --latest"
  if $DRY_RUN; then
    echo "dry run: nothing changed."
    exit 0
  fi

  if ! $ASSUME_YES; then
    [ -t 0 ] || { echo "refusing to roll back without a terminal; re-run with --yes if this is intended" >&2; exit 1; }
    printf 'type %s to confirm the rollback: ' "$TAG"
    read -r answer
    [ "$answer" = "$TAG" ] || { echo "stopped; nothing changed"; exit 0; }
  fi
  gh auth status >/dev/null 2>&1 || { echo "gh is not authenticated" >&2; exit 1; }
  gh release edit "$TAG" --repo "$SLUG" --latest
  if ! wait_for_feed "https://github.com/$SLUG/releases/latest/download/appcast.xml" "$WANT"; then
    echo "$TAG is latest, but the feed does not serve build $WANT yet (CDN cache?); check by hand" >&2
    exit 1
  fi
  echo "rolled back: latest is $TAG and the feed serves build $WANT"
  exit 0
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' ../crates/qh-ffi/Cargo.toml)"
VERSION="${VERSION:-0.2.0}"
TAG="v$VERSION"
SPARKLE_BIN=".build/artifacts/sparkle/Sparkle/bin"

# Committed beside the code it describes, so a release's own account of itself is reviewable and
# comes back with `git show v<version>:docs/releases/v<version>.md`.
if [ -z "${RELEASE_NOTES:-}" ] && [ -f "../docs/releases/$TAG.md" ]; then
  RELEASE_NOTES="../docs/releases/$TAG.md"
fi

# One entry today; kept as an array so adding a second Sparkle tool stays a one-line change
# without tripping SC2043, which flags a literal single-item `for` list.
sparkle_tools=(generate_appcast)
for tool in "${sparkle_tools[@]}"; do
  if [ ! -x "$SPARKLE_BIN/$tool" ]; then
    echo "$SPARKLE_BIN/$tool is missing — run 'swift package resolve' first" >&2
    exit 1
  fi
done

# ---------------------------------------------------------------- guards
# Everything below commits this machine to a published release, so each guard names the thing that
# would make the release wrong rather than just refusing.

if [ "$(git rev-parse --abbrev-ref HEAD)" != "main" ]; then
  echo "not on main (on $(git rev-parse --abbrev-ref HEAD)): a release is tagged on main" >&2
  exit 1
fi

if [ -n "$(git status --porcelain)" ]; then
  if $DRY_RUN; then
    # Nothing is published in a dry run, and wanting to see the packaging work on a tree you are
    # still editing is the point of it. Said out loud, because what comes out of this run is then
    # not what any tag would describe.
    echo "note: uncommitted changes — a dry run packages them anyway, a real release would not"
    git status --short | sed 's/^/      /'
  else
    echo "uncommitted changes: the tag would describe a commit that is not what was built" >&2
    git status --short >&2
    exit 1
  fi
fi

if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  # Not fatal in a dry run, which publishes nothing; fatal in a real one, where it would either
  # fail halfway or move a tag that is already out there.
  if $DRY_RUN; then
    echo "note: tag $TAG already exists — bump the version in crates/qh-ffi/Cargo.toml to release"
  else
    echo "tag $TAG already exists — bump the version in crates/qh-ffi/Cargo.toml" >&2
    exit 1
  fi
fi

if ! $DRY_RUN && ! $ASSUME_YES; then
  gh auth status >/dev/null 2>&1 || { echo "gh is not authenticated" >&2; exit 1; }
fi

SLUG="$(origin_slug)"
FEED_URL="https://github.com/$SLUG/releases/latest/download/appcast.xml"
DOWNLOAD_PREFIX="https://github.com/$SLUG/releases/latest/download/"

# ---------------------------------------------------------------- package
echo "==> packaging $VERSION"
./build-dmg.sh "${DMG_ARGS[@]+"${DMG_ARGS[@]}"}"
BUILD="$("$PLISTBUDDY" -c "Print :CFBundleVersion" dist/QueryHive.app/Contents/Info.plist)"
DMG="dist/QueryHive-$VERSION-arm64.dmg"
[ -f "$DMG" ] || { echo "build-dmg.sh did not produce $DMG" >&2; exit 1; }

# A release folder holding exactly one archive, because generate_appcast writes an item per
# archive it finds and every one of those items would carry the same `latest/download/` URL.
RELEASE_DIR="dist/release"
rm -rf "$RELEASE_DIR"
mkdir -p "$RELEASE_DIR"
cp "$DMG" "$RELEASE_DIR/"
./"$SPARKLE_BIN/generate_appcast" \
  --download-url-prefix "$DOWNLOAD_PREFIX" \
  -o "$RELEASE_DIR/appcast.xml" "$RELEASE_DIR"
ASSET="$RELEASE_DIR/$(basename "$DMG")"

# The build number is what Sparkle compares. One that is not greater than what the live feed
# already offers is a release nobody is told about, which is worth catching here rather than
# wondering about later.
LIVE="$(served_build "$FEED_URL")"
# A value that is not a number cannot be compared, and `[ -le ]` on one fails quietly inside an
# `&&` list, which would skip this guard. Refuse instead of guessing.
case "$BUILD" in ''|*[!0-9]*) echo "the packaged build number '$BUILD' is not numeric" >&2; exit 1 ;; esac
case "$LIVE" in
  *[!0-9]*) echo "the live feed's build number '$LIVE' is not numeric, so it cannot be compared; check $FEED_URL" >&2; exit 1 ;;
esac
if [ -n "$LIVE" ] && [ "$BUILD" -le "$LIVE" ]; then
  echo "build $BUILD is not newer than the live feed's $LIVE — the app would ignore this release" >&2
  exit 1
fi

echo
echo "version   $VERSION   build $BUILD"
echo "dmg       $ASSET ($(du -h "$ASSET" | cut -f1), sha256 $(shasum -a 256 "$ASSET" | cut -d' ' -f1))"
echo "appcast   $RELEASE_DIR/appcast.xml"
echo "feed      $FEED_URL"
echo "tag       $TAG  (behind it: $(git rev-parse --short HEAD))"
echo
echo "publishing would run:"
echo "  git tag -a $TAG -m \"QueryHive $VERSION\""
echo "  git push origin main && git push origin $TAG"
if [ -n "${RELEASE_NOTES:-}" ]; then
  echo "  gh release create $TAG $ASSET $RELEASE_DIR/appcast.xml --title \"QueryHive $VERSION\" --notes-file $RELEASE_NOTES --latest"
else
  echo "  gh release create $TAG $ASSET $RELEASE_DIR/appcast.xml --title \"QueryHive $VERSION\" --generate-notes --latest"
fi

if $DRY_RUN; then
  echo
  echo "dry run: nothing tagged, pushed or published."
  exit 0
fi

# ---------------------------------------------------------------- publish
if ! $ASSUME_YES; then
  printf 'publish %s to %s? [y/N] ' "$TAG" "$SLUG"
  read -r answer
  case "$answer" in
    y|Y|yes|YES) ;;
    *) echo "stopped before tagging; $ASSET is built and left in place"; exit 0 ;;
  esac
fi

NOTES="${RELEASE_NOTES:-}"
if [ -n "$NOTES" ] && [ ! -f "$NOTES" ]; then
  echo "RELEASE_NOTES points at $NOTES, which does not exist" >&2
  exit 1
fi

git tag -a "$TAG" -m "QueryHive $VERSION"
git push origin main
git push origin "$TAG"

NOTES_ARGS=(--generate-notes)
if [ -n "$NOTES" ]; then
  NOTES_ARGS=(--notes-file "$NOTES")
fi

RELEASE_URL="$(gh release create "$TAG" "$ASSET" "$RELEASE_DIR/appcast.xml" \
  --title "QueryHive $VERSION" "${NOTES_ARGS[@]}" --latest)"

# ---------------------------------------------------------------- verify
# The feed is the part that fails silently: a release without the appcast asset still looks
# published, and the only symptom is an app that never offers an update. So ask the URL the app
# has in its Info.plist, and check that it serves *this* build.
#
# Asking once is not enough. GitHub serves release downloads through a CDN, and immediately after
# a release appears the redirect can still be a cached miss — measured while cutting this very
# release, the exact URL answered 404 for minutes while the API already reported the release as
# latest. So it is retried, with the app's own URL first because that is the one that has to work;
# only if every attempt fails does it try the same URL with a query string, which is what tells a
# stale cache entry apart from a release that is genuinely missing its appcast.
PACKAGED="$("$PLISTBUDDY" -c "Print :SUFeedURL" dist/QueryHive.app/Contents/Info.plist)"
if wait_for_feed "$PACKAGED" "$BUILD"; then SERVED="$BUILD"; else SERVED="$(served_build "$PACKAGED")"; fi

if [ "$SERVED" != "$BUILD" ]; then
  if [ "$(served_build "$PACKAGED?cachebust=$BUILD")" = "$BUILD" ]; then
    # The release is right; one CDN entry is behind. Nothing to redo and nothing to fix by hand,
    # so this is a warning rather than a failure — and it says so, because the alternative is
    # somebody re-uploading assets that were never wrong.
    echo
    echo "released $TAG"
    echo "  $RELEASE_URL"
    echo "  note: the CDN still holds an older answer for the exact feed URL — it expires on its own"
    echo "  (the same URL with a cache-busting query already serves build $BUILD)"
    exit 0
  fi
  echo "the feed at $PACKAGED serves build '${SERVED:-nothing}', not $BUILD" >&2
  echo "the release exists; the app will not see it until the appcast asset is in place" >&2
  exit 1
fi

echo
echo "released $TAG"
echo "  $RELEASE_URL"
echo "  feed serves build $SERVED (what the app will find)"
