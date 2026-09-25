#!/usr/bin/env bash
# Builds the Rust engine's static archive and regenerates app/Generated/ from it: the Swift
# bindings the app links against, and the C module they call into.
#
#   ./build-ffi.sh        # release library, then bindings
#   ./build-ffi.sh debug  # debug library instead (faster to build, ~4x bigger)
#
# Two artifacts come out of one `cargo build`. The generator reads the `cdylib`
# (target/<profile>/libqh_ffi.dylib) because it has to see a compiled library; the app links
# the `staticlib` (target/<profile>/libqh_ffi.a), staged into target/ffi/static/ so the two
# can never be confused for one another. Only the second ends up inside the bundle.
#
# app/Generated/ is committed, and regenerating it is a visible diff on purpose: the FFI surface
# is a contract the app is written against (`DatabaseEngine`, blueprint §1.6), so a change to it
# should show up in review as a diff in the app's own tree rather than as a compile error on
# whichever machine happens to regenerate first. Blueprint §8 plans the same thing ("Binding …
# di-commit, diuji CI agar tidak basi").
#
# The generator is this workspace's own pinned `uniffi`, never a `cargo install`ed one: the
# generator and the runtime in Cargo.lock have to be the same version or the bindings do not
# compile against the library they were generated for
# (`crates/qh-ffi/src/bin/uniffi-bindgen.rs` says the same thing from the Rust side).
#
# Why this is a script and not a SwiftPM plugin: `swift build` has no pre-build hook, so the
# artifact has to exist before it links, and a plugin cannot write into `target/` for a package
# whose own sources are what gets built. The blueprint's XCFramework would carry this as a
# plugin, but it is not built and not needed while the app is arm64-only: an archive plus the
# framework list in `app/Package.swift` is enough, and it is one fewer artifact to keep in step.
set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(dirname "$(pwd)")"
PROFILE="${1:-release}"
# Absolute, not a bare `Generated`: the generator runs inside a subshell that has cd'd to
# $ROOT (it needs the workspace's Cargo.toml), and `--out-dir` is resolved against that
# subshell's cwd. A relative value lands in a stray `Generated/` beside target/ at the repo
# root instead, and the renames below then fail with "No such file or directory".
GENERATED="$(pwd)/Generated"
LIBRARY="$ROOT/target/$PROFILE/libqh_ffi.dylib"
ARCHIVE="$ROOT/target/$PROFILE/libqh_ffi.a"
# Where `app/Package.swift` links from, and the reason it is its own directory rather
# than `target/$PROFILE`: a linker handed `-L target/release -lqh_ffi` finds
# `libqh_ffi.dylib` beside the archive and prefers the dylib, which puts the absolute
# install name straight back into the app. A directory holding the archive and nothing
# else leaves `ld` no choice. Kept per profile so that a debug build cannot be linked
# into a release app: `Package.swift` tries `release` first, exactly as it did when it
# was looking for the dylib. `target/` is gitignored, so staging adds no file to the
# tree.
STATIC_DIR="$ROOT/target/ffi/static/$PROFILE"

log() { echo "[build-ffi] $*"; }
die() { echo "[build-ffi] error: $*" >&2; exit 1; }

case "$PROFILE" in
  release|debug) ;;
  *) die "unknown profile '$PROFILE' (expected release or debug)" ;;
esac

log "cargo build -p qh-ffi --profile $PROFILE"
(cd "$ROOT" && cargo build -p qh-ffi --profile "$PROFILE")
[ -f "$LIBRARY" ] || die "cargo reported success but $LIBRARY is missing"
[ -f "$ARCHIVE" ] || die "cargo reported success but $ARCHIVE is missing (is 'staticlib' still in the crate-type list?)"

# Stage the archive for the linker. Cleared first so that a build that stops somewhere
# above cannot leave the previous build's archive looking current, and so that a stray
# `libqh_ffi.dylib` from an older layout cannot make the static link a dynamic one.
log "stage $(basename "$ARCHIVE") in ${STATIC_DIR#"$ROOT"/}"
rm -rf "$STATIC_DIR"
mkdir -p "$STATIC_DIR"
cp "$ARCHIVE" "$STATIC_DIR/"

# `--library` on the compiled dylib rather than on the source: it is the only way the generator
# can see the types the proc macros produced, and the bindings' checksums have to match what the
# library was compiled with.
log "uniffi-bindgen generate --library"
(cd "$ROOT" && cargo run -q -p qh-ffi --bin uniffi-bindgen -- \
  generate --library "$LIBRARY" --language swift --out-dir "$GENERATED")

# The generator writes the three files under names of its own (`qh_ffi.swift`, `qh_ffiFFI.h`,
# `qh_ffiFFI.modulemap`). They are moved into the two SwiftPM targets Package.swift declares,
# because SwiftPM requires a target's files to sit under one directory.
mv "$GENERATED/qh_ffi.swift" "$GENERATED/QueryHiveFFI/qh_ffi.swift"
mv "$GENERATED/qh_ffiFFI.h" "$GENERATED/qh_ffiFFI/qh_ffiFFI.h"
mv "$GENERATED/qh_ffiFFI.modulemap" "$GENERATED/qh_ffiFFI/module.modulemap"

# A header-only clang target emits no object file, but SwiftPM still lists
# `qh_ffiFFI.o` as a link input for anything that depends on it, so the link fails
# from a clean build. Give the target one translation unit; written here rather than
# committed by hand so regeneration cannot silently drop it.
cat > "$GENERATED/qh_ffiFFI/qh_ffiFFI.c" <<'EOF'
// This file exists because a SwiftPM clang target with only a header and a modulemap
// produces no object file, yet the linker still expects `<target>.o` whenever another
// target (the executable, or the test bundle that embeds it) links against it. One
// translation unit that includes the generated header gives clang something to emit.
// app/build-ffi.sh rewrites it on every regeneration, so it survives `./build-ffi.sh`.
#include "qh_ffiFFI.h"
EOF

log "regenerated $(cd "$GENERATED" && ls QueryHiveFFI qh_ffiFFI | tr '\n' ' ')"
