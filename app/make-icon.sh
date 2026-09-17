#!/bin/bash
# Regenerates assets/icon.icns from the app's own drawing code.
#
#   ./app/make-icon.sh
#
# The icon is not a file someone drew once and checked in: it is rendered by QueryHiveIcon in
# Support/AppIcon.swift, from the same hexagon lattice as the in-app mark. Editing the artwork
# means editing that view and re-running this, which is the only way the Dock and the sidebar can
# be guaranteed to agree.
set -euo pipefail
cd "$(dirname "$0")"

OUT="../assets/icon.icns"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

swift build -c release --disable-sandbox
BIN="$(swift build -c release --disable-sandbox --show-bin-path)/QueryHive"

# Two artworks, because one does not survive the whole range: the seven-cell comb turns to a
# smudge at 16 and 32 (measured -- at 32 only the lit centre cell was left), so those sizes get
# the single hexagonal cell instead.
"$BIN" --icon "$WORK/full-1024.png"
"$BIN" --icon "$WORK/compact-1024.png" --icon-compact

ICONSET="$WORK/QueryHive.iconset"
mkdir -p "$ICONSET"

# name:source:size -- every entry Apple's icon grid asks for.
resize() { sips -z "$3" "$3" "$WORK/$2-1024.png" --out "$ICONSET/$1" >/dev/null; }
resize icon_16x16.png      compact 16
resize icon_16x16@2x.png   compact 32
resize icon_32x32.png      compact 32
resize icon_32x32@2x.png   full    64
resize icon_128x128.png    full    128
resize icon_128x128@2x.png full    256
resize icon_256x256.png    full    256
resize icon_256x256@2x.png full    512
resize icon_512x512.png    full    512
resize icon_512x512@2x.png full    1024

iconutil -c icns "$ICONSET" -o "$OUT"
echo "wrote $OUT ($(du -h "$OUT" | cut -f1))"
echo
echo "preview with:  sips -s format png $OUT --out /tmp/icon.png && open /tmp/icon.png"
