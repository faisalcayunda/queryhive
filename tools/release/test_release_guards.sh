#!/bin/bash
# Tests the guards and the --rollback mode of app/release.sh without releasing anything.
#
#   tools/release/test_release_guards.sh
#
# Every case runs a copy of release.sh in a throwaway git repo on a PATH whose `gh`, `curl`,
# `sleep` and `git push` are stubs, so nothing can reach GitHub, tag the real repository or upload
# a file. The stubs record what they were asked to do in $STUB_LOG, and the refusal cases assert
# that no publishing call was made. Needs only bash, git and coreutils (macOS or Ubuntu).
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="$HERE/../../app/release.sh"
REAL_GIT="$(command -v git)"
ORIG_PATH="$PATH"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

PASS=0
FAIL=0

# ---------------------------------------------------------------- stubs (shared by all cases)
BIN="$TMP/bin"
mkdir -p "$BIN"

cat >"$BIN/gh" <<'STUB'
#!/bin/bash
echo "gh $*" >>"$STUB_LOG"
case "$1 $2" in
  "auth status") exit "${STUB_GH_AUTH:-0}" ;;
  "release view")
    if [[ "$*" == *"--json tagName"* ]]; then echo "${STUB_LATEST:-v0.2.0}"; exit 0; fi
    [ -f "$STUB_STATE/view_$3" ] || { echo "release not found" >&2; exit 1; }
    cat "$STUB_STATE/view_$3" ;;
  "release edit"|"release create") touch "$STUB_STATE/changed"; echo "https://example.invalid/release" ;;
  *) echo "gh stub: unexpected call: $*" >&2; exit 99 ;;
esac
STUB

cat >"$BIN/curl" <<'STUB'
#!/bin/bash
url="${*: -1}"
echo "curl $url" >>"$STUB_LOG"
case "$url" in
  */releases/download/*/appcast.xml) b="${STUB_TAG_APPCAST_BUILD:-}" ;;
  */releases/latest/download/appcast.xml*)
    if [ -f "$STUB_STATE/changed" ]; then b="${STUB_FEED_AFTER:-}"; else b="${STUB_FEED_BUILD:-}"; fi ;;
  *) echo "curl stub: unexpected url: $url" >&2; exit 99 ;;
esac
[ -n "$b" ] || exit 22
echo "<item><sparkle:version>$b</sparkle:version></item>"
STUB

cat >"$BIN/git" <<STUB
#!/bin/bash
if [ "\$1" = push ]; then echo "git \$*" >>"\$STUB_LOG"; exit 0; fi
exec "$REAL_GIT" "\$@"
STUB

printf '#!/bin/bash\nexit 0\n' >"$BIN/sleep"

cat >"$BIN/plistbuddy" <<'STUB'
#!/bin/bash
case "$2" in
  "Print :CFBundleVersion") echo "${STUB_BUILD:-200}" ;;
  "Print :SUFeedURL") echo "https://github.com/test/queryhive/releases/latest/download/appcast.xml" ;;
  *) exit 1 ;;
esac
STUB

chmod +x "$BIN"/*
PATH="$BIN:$ORIG_PATH"
[ "$(command -v gh)" = "$BIN/gh" ] || { echo "refusing to run: gh is not the stub" >&2; exit 1; }

export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export PLISTBUDDY="$BIN/plistbuddy"

# ---------------------------------------------------------------- one sandbox per case
sandbox() {
  ROOT="$TMP/case"
  rm -rf "$ROOT"
  mkdir -p "$ROOT/app/.build/artifacts/sparkle/Sparkle/bin" "$ROOT/crates/qh-ffi"
  cp "$SRC" "$ROOT/app/release.sh"
  printf 'version = "0.1.0"\n' >"$ROOT/crates/qh-ffi/Cargo.toml"
  printf 'dist/\n' >"$ROOT/.gitignore"
  cat >"$ROOT/app/build-dmg.sh" <<'STUB'
#!/bin/bash
mkdir -p dist/QueryHive.app/Contents
echo plist >dist/QueryHive.app/Contents/Info.plist
echo dmg >dist/QueryHive-0.1.0-arm64.dmg
STUB
  cat >"$ROOT/app/.build/artifacts/sparkle/Sparkle/bin/generate_appcast" <<'STUB'
#!/bin/bash
while [ $# -gt 0 ]; do [ "$1" = -o ] && out="$2"; shift; done
echo "<rss/>" >"$out"
STUB
  chmod +x "$ROOT/app/release.sh" "$ROOT/app/build-dmg.sh" "$ROOT/app/.build/artifacts/sparkle/Sparkle/bin/generate_appcast"
  (
    cd "$ROOT" || exit 1
    git init -q -b main
    git config user.name tester
    git config user.email tester@example.invalid
    git remote add origin https://github.com/test/queryhive.git
    git add -A
    git commit -q -m init
  )
  export STUB_LOG="$TMP/log" STUB_STATE="$TMP/state"
  rm -rf "$STUB_STATE"
  mkdir -p "$STUB_STATE"
  : >"$STUB_LOG"
  unset STUB_GH_AUTH STUB_LATEST STUB_TAG_APPCAST_BUILD STUB_FEED_BUILD STUB_FEED_AFTER STUB_BUILD
}

# view <tag> <draft> <prerelease> <published|none> <assets>
view() { printf '%s\t%s\t%s\t%s\n' "$2" "$3" "$4" "$5" >"$STUB_STATE/view_$1"; }
GOOD_ASSETS="appcast.xml,QueryHive-0.1.0-arm64.dmg"

tag_exists() { "$REAL_GIT" -C "$ROOT" rev-parse -q --verify refs/tags/v0.1.0 >/dev/null && echo yes || echo no; }

# run [--stdin TEXT] -- args...  -> OUT, RC
run() {
  local input=""
  tag_exists >"$TMP/tag_before"
  if [ "${1:-}" = --stdin ]; then input="$2"; shift 2; fi
  OUT="$(cd "$ROOT/app" && ./release.sh "$@" 2>&1 <<<"$input")"
  RC=$?
}

# check <name> <want rc> <pattern in output> [nopub]  — nopub: nothing pushed, edited or tagged
check() {
  local name="$1" want="$2" pat="$3" nopub="${4:-}" ok=true
  [ "$RC" -eq "$want" ] || ok=false
  printf '%s' "$OUT" | /usr/bin/grep -q -- "$pat" || ok=false
  if [ "$nopub" = nopub ]; then
    if /usr/bin/grep -Eq 'git push|release (edit|create)' "$STUB_LOG"; then ok=false; fi
    [ "$(tag_exists)" = "$(cat "$TMP/tag_before")" ] || ok=false
  fi
  if $ok; then
    PASS=$((PASS + 1)); echo "ok   $name"
  else
    FAIL=$((FAIL + 1)); echo "FAIL $name (rc $RC, wanted $want, pattern '$pat')"
    printf '%s\n' "$OUT" | sed 's/^/       | /'
  fi
}

# ---------------------------------------------------------------- release guards
sandbox
run --bogus
check "unknown option is refused" 2 "unknown option"

sandbox; "$REAL_GIT" -C "$ROOT" checkout -q -b topic
run --dry-run
check "refuses to release off main" 1 "not on main" nopub

sandbox; echo x >"$ROOT/app/dirty"
run --yes
check "refuses a dirty tree" 1 "uncommitted changes" nopub
run --dry-run
check "a dry run only notes a dirty tree" 0 "dry run: nothing tagged"

sandbox; "$REAL_GIT" -C "$ROOT" tag v0.1.0
run --yes
check "refuses an existing tag" 1 "already exists" nopub
run --dry-run
check "a dry run only notes an existing tag" 0 "dry run: nothing tagged"

sandbox; STUB_GH_AUTH=1 run
check "refuses without gh authentication" 1 "gh is not authenticated" nopub
export -n STUB_GH_AUTH

sandbox; export STUB_FEED_BUILD=100 STUB_BUILD=200
run --dry-run
check "a newer build than the live feed passes" 0 "dry run: nothing tagged"
export STUB_BUILD=100
run --dry-run
check "an equal build number is refused" 1 "not newer than the live feed" nopub
export STUB_BUILD=99
run --dry-run
check "an older build number is refused" 1 "not newer than the live feed" nopub

# The bug this file was written for: a live build that is not a number made `[ -le ]` fail inside
# an && list, and the guard skipped itself without a word.
sandbox; export STUB_FEED_BUILD=abc STUB_BUILD=200
run --dry-run
check "a non-numeric live build refuses instead of skipping the guard" 1 "not numeric" nopub
export STUB_FEED_BUILD=2.0.1
run --dry-run
check "a dotted live build refuses too" 1 "not numeric" nopub
export STUB_FEED_BUILD="" STUB_BUILD=200
run --dry-run
check "an unreachable feed (first release) passes" 0 "dry run: nothing tagged"
export STUB_FEED_BUILD=100 STUB_BUILD=abc
run --dry-run
check "a non-numeric packaged build refuses" 1 "packaged build number" nopub

sandbox; export STUB_FEED_BUILD=100 STUB_BUILD=200
run --stdin n
check "declining the prompt stops before tagging" 0 "stopped before tagging" nopub

sandbox; export STUB_FEED_BUILD=100 STUB_BUILD=200 STUB_FEED_AFTER=200
run --yes
check "a full run publishes and sees the feed" 0 "feed serves build 200"
/usr/bin/grep -q 'git push origin main' "$STUB_LOG" && /usr/bin/grep -q 'release create v0.1.0' "$STUB_LOG" \
  && { PASS=$((PASS + 1)); echo "ok   the publish steps went to the stubs"; } \
  || { FAIL=$((FAIL + 1)); echo "FAIL the publish steps went to the stubs"; }

sandbox; export STUB_FEED_BUILD=100 STUB_BUILD=200 STUB_FEED_AFTER=100
run --yes
check "a feed that never serves the new build fails the release" 1 "serves build '100', not 200"

# ---------------------------------------------------------------- --rollback
sandbox
run --rollback
check "rollback without a target is refused" 2 "explicit tag" nopub
run --rollback --yes
check "rollback never takes a flag for its target" 2 "explicit tag" nopub
[ ! -s "$STUB_LOG" ] && { PASS=$((PASS + 1)); echo "ok   a refused rollback made no gh call"; } \
  || { FAIL=$((FAIL + 1)); echo "FAIL a refused rollback made no gh call"; }
run --rollback latest
check "rollback needs a vX.Y.Z tag" 2 "not a stable vX.Y.Z" nopub
run --rollback v0.1
check "rollback rejects a short version" 2 "not a stable vX.Y.Z" nopub
run --rollback v0.1.0 --notarize
check "rollback rejects build options" 2 "does not apply" nopub

sandbox; export STUB_LATEST=v0.2.0 STUB_TAG_APPCAST_BUILD=150 STUB_FEED_AFTER=150
view v0.1.0 false false 2026-10-01T00:00:00Z "$GOOD_ASSETS"
run --rollback v0.1.0 --dry-run
check "a valid rollback dry run prints the command" 0 "would run: gh release edit v0.1.0 --repo test/queryhive --latest" nopub
run --rollback v0.1.0
check "rollback without a terminal or --yes is refused" 1 "without a terminal" nopub
STUB_GH_AUTH=1 run --rollback v0.1.0 --yes
check "rollback needs gh authentication" 1 "gh is not authenticated" nopub
export -n STUB_GH_AUTH
run --rollback v0.1.0 --yes
check "rollback --yes edits latest and sees the feed" 0 "rolled back: latest is v0.1.0"
/usr/bin/grep -q 'gh release edit v0.1.0 --repo test/queryhive --latest' "$STUB_LOG" \
  && { PASS=$((PASS + 1)); echo "ok   the edit went to the stub gh"; } \
  || { FAIL=$((FAIL + 1)); echo "FAIL the edit went to the stub gh"; }

sandbox; export STUB_LATEST=v0.2.0 STUB_TAG_APPCAST_BUILD=150 STUB_FEED_AFTER=300
view v0.1.0 false false 2026-10-01T00:00:00Z "$GOOD_ASSETS"
run --rollback v0.1.0 --yes
check "rollback fails when the feed does not serve the target" 1 "does not serve build 150"

sandbox; export STUB_TAG_APPCAST_BUILD=150
export STUB_LATEST=v0.2.0
view v0.2.0 false false 2026-10-01T00:00:00Z "$GOOD_ASSETS"
run --rollback v0.2.0 --dry-run
check "rolling back to the current latest is refused" 1 "not older" nopub
view v0.3.0 false false 2026-10-01T00:00:00Z "$GOOD_ASSETS"
run --rollback v0.3.0 --dry-run
check "rolling back to a newer release is refused" 1 "not older" nopub
export STUB_LATEST=v0.10.0
view v0.9.0 false false 2026-10-01T00:00:00Z "QueryHive-0.9.0-arm64.dmg,appcast.xml"
run --rollback v0.9.0 --dry-run
check "versions compare numerically (0.9.0 is older than 0.10.0)" 0 "would run" nopub
export STUB_LATEST=v0.2.0
view v0.1.0 true false none "$GOOD_ASSETS"
run --rollback v0.1.0 --dry-run
check "a draft is refused" 1 "published stable release" nopub
view v0.1.0 false true 2026-10-01T00:00:00Z "$GOOD_ASSETS"
run --rollback v0.1.0 --dry-run
check "a prerelease is refused" 1 "published stable release" nopub
view v0.0.1 false false 2026-09-01T00:00:00Z "QueryHive-0.0.1-arm64.dmg"
run --rollback v0.0.1 --dry-run
check "a release without an appcast is refused (v0.0.1 today)" 1 "no appcast.xml" nopub
view v0.1.0 false false 2026-10-01T00:00:00Z "appcast.xml"
run --rollback v0.1.0 --dry-run
check "a release without the dmg is refused" 1 "no QueryHive-0.1.0-arm64.dmg" nopub
run --rollback v0.0.9 --dry-run
check "an unknown release is refused" 1 "not a release of" nopub
view v0.1.0 false false 2026-10-01T00:00:00Z "$GOOD_ASSETS"
export STUB_TAG_APPCAST_BUILD=""
run --rollback v0.1.0 --dry-run
check "an unreadable appcast is refused" 1 "numeric build number" nopub
export STUB_TAG_APPCAST_BUILD=x1
run --rollback v0.1.0 --dry-run
check "a non-numeric appcast build is refused" 1 "numeric build number" nopub

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
