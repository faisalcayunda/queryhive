#!/usr/bin/env bash
# Builds the bundled Python engine used by the QueryHive app:
#   app/.engine/python/          standalone CPython, arm64, relocatable
#   app/.engine/site-packages/   pinned packages installed with --target
#
# Idempotent: re-running this script is a fast no-op unless the pinned Python
# version, app/engine-requirements.txt, or this script itself changed.
set -euo pipefail

# --- fixed pins ---------------------------------------------------------
# Exact CPython version, arm64 only. Source: uv-managed Python, which ships the
# same relocatable "install_only" python-build-standalone builds that
# python-build-standalone publishes directly (see `uv python dir`).
#
# Stock (GIL) 3.12 on purpose. The repository's own venv_setup.sh prefers a
# free-threaded 3.14t because the row renderer scales across threads without the
# GIL (2.8x vs 1.3x on a fast coordinator), but orjson refuses to build there and
# trino imports it when present. The export still overlaps its Trino fetch with
# the writer on any build, which is where most of the wall-clock win comes from.
# To try 3.14t: set PYTHON_VERSION=3.14.x, PYTHON_DIST_NAME to the matching
# "cpython-<version>-macos-aarch64-none" directory and add trino with --no-deps.
PYTHON_VERSION="3.12.12"
PYTHON_PLATFORM="aarch64-apple-darwin"
PYTHON_DIST_NAME="cpython-${PYTHON_VERSION}-macos-aarch64-none"

# Top-level package set the engine is built from (kept here so the
# requirements header and the actual install step can never drift apart).
# trino is left unpinned so the compile step resolves its own dependency tree;
# the generated lockfile records the exact versions, which is what makes the
# next build reproducible.
TOP_LEVEL_REQS=(
  'trino>=0.330'
  'openpyxl==3.1.5'
  'xlwt==1.3.0'
)

# --- paths ---------------------------------------------------------------
SCRIPT_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENGINE_DIR="$APP_DIR/.engine"
STAMP_FILE="$ENGINE_DIR/.build-stamp"
REQ_FILE="$APP_DIR/engine-requirements.txt"

log() { echo "[build-engine] $*"; }
die() { echo "[build-engine] error: $*" >&2; exit 1; }

command -v uv >/dev/null 2>&1 || die "uv is required (https://docs.astral.sh/uv/) and was not found on PATH"

# --- 1. requirements lockfile (generated once, then reused as-is) --------
# app/engine-requirements.txt is a pinned lockfile: once it exists it is
# read verbatim on every run. Delete it (or edit the pins above) and rerun
# this script to regenerate it.
generate_requirements() {
  log "generating $REQ_FILE"
  local body
  body="$(printf '%s\n' "${TOP_LEVEL_REQS[@]}" | uv pip compile \
    --python-version 3.12 --python-platform "$PYTHON_PLATFORM" \
    --only-binary :all: --no-header --no-annotate - 2>/dev/null)" \
    || die "uv pip compile failed while generating $REQ_FILE"
  [ -n "$body" ] || die "uv pip compile produced no output"
  {
    echo "# Fully pinned for Python 3.12 on macOS arm64 (aarch64-apple-darwin)."
    echo "# Generated from this top-level set:"
    for r in "${TOP_LEVEL_REQS[@]}"; do echo "#   $r"; done
    echo "#"
    echo "# Regenerate with:"
    echo "#   printf '%s\\n' \\"
    for r in "${TOP_LEVEL_REQS[@]}"; do echo "#     '$r' \\"; done
    echo "#   | uv pip compile --python-version 3.12 --python-platform $PYTHON_PLATFORM \\"
    echo "#       --only-binary :all: --no-header --no-annotate -"
    echo "#"
    echo "$body"
  } > "$REQ_FILE"
}

[ -f "$REQ_FILE" ] || generate_requirements

# --- 2. idempotency stamp -------------------------------------------------
current_hash() {
  { printf 'python_version=%s\n' "$PYTHON_VERSION"; cat "$REQ_FILE"; cat "$SCRIPT_PATH"; } \
    | /usr/bin/shasum -a 256 | /usr/bin/awk '{print $1}'
}
HASH="$(current_hash)"

if [ -f "$STAMP_FILE" ] && [ "$(cat "$STAMP_FILE")" = "$HASH" ] \
  && [ -x "$ENGINE_DIR/python/bin/python3" ] && [ -d "$ENGINE_DIR/site-packages" ]; then
  log "app/.engine is up to date (stamp matches), nothing to do"
  exit 0
fi

log "building app/.engine (Python $PYTHON_VERSION, $(basename "$REQ_FILE") $(/usr/bin/shasum -a 256 "$REQ_FILE" | cut -c1-12))"

# --- 3. fetch the standalone interpreter ----------------------------------
uv python install "$PYTHON_VERSION" >/dev/null || die "uv python install $PYTHON_VERSION failed"
UV_PYTHON_DIR="$(uv python dir)"
SRC_PYTHON="$UV_PYTHON_DIR/$PYTHON_DIST_NAME"
[ -d "$SRC_PYTHON" ] || die "expected uv-managed Python at $SRC_PYTHON, not found"
"$SRC_PYTHON/bin/python3.12" -c "import sys; assert sys.version.startswith('$PYTHON_VERSION'), sys.version" \
  || die "uv-managed Python at $SRC_PYTHON is not version $PYTHON_VERSION"

rm -rf "$ENGINE_DIR"
mkdir -p "$ENGINE_DIR"

# Copy with symlinks resolved (-L) so the tree is self-contained and does
# not depend on anything under ~/.local/share/uv.
cp -RL "$SRC_PYTHON" "$ENGINE_DIR/python"
BEFORE_PYTHON="$(du -sk "$ENGINE_DIR/python" | cut -f1)"

# --- 4. install pinned packages -------------------------------------------
mkdir -p "$ENGINE_DIR/site-packages"
uv pip install \
  --target "$ENGINE_DIR/site-packages" \
  --python "$ENGINE_DIR/python/bin/python3.12" \
  --only-binary :all: \
  -r "$REQ_FILE" \
  || die "uv pip install into $ENGINE_DIR/site-packages failed"
BEFORE_SITE="$(du -sk "$ENGINE_DIR/site-packages" | cut -f1)"

# --- 5. trim ---------------------------------------------------------------
# Stdlib pieces the engine never needs.
for d in idlelib tkinter turtledemo ensurepip test; do
  rm -rf "$ENGINE_DIR/python/lib/python3.12/$d"
done
rm -f "$ENGINE_DIR/python/lib/python3.12/lib-dynload"/_tkinter*.so

# Test suites bundled inside installed packages (only ones actually present).
for d in "$ENGINE_DIR/site-packages"/*/tests; do
  [ -d "$d" ] && rm -rf "$d"
done

# Bytecode caches (never needed: the app runs with PYTHONDONTWRITEBYTECODE=1).
/usr/bin/find "$ENGINE_DIR" -type d -name "__pycache__" -print0 | xargs -0 rm -rf

AFTER_PYTHON="$(du -sk "$ENGINE_DIR/python" | cut -f1)"
AFTER_SITE="$(du -sk "$ENGINE_DIR/site-packages" | cut -f1)"

# --- 6. verify the trimmed engine actually still works ---------------------
# PYTHONDONTWRITEBYTECODE avoids re-creating __pycache__ we just trimmed
# (matches the contract launch flags the app itself uses).
PYTHONPATH="$ENGINE_DIR/site-packages" PYTHONDONTWRITEBYTECODE=1 "$ENGINE_DIR/python/bin/python3" -s -c "
import trino, openpyxl, xlwt
import trino.auth
import trino.dbapi
" || die "post-trim import check failed, engine left in place for inspection (stamp not written)"

# --- 7. stamp ----------------------------------------------------------------
echo "$HASH" > "$STAMP_FILE"

log "python: ${BEFORE_PYTHON}K -> ${AFTER_PYTHON}K"
log "site-packages: ${BEFORE_SITE}K -> ${AFTER_SITE}K"
log "done"
