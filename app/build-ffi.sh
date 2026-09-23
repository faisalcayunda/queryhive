#!/usr/bin/env bash
# Builds the Rust engine's shared library and regenerates app/Generated/ from it: the Swift
# bindings the app links against, and the C module they call into.
#
#   ./build-ffi.sh        # release library, then bindings
#   ./build-ffi.sh debug  # debug library instead (faster to build, ~4x bigger)
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
# library has to exist before it links, and the plugin that would do this belongs with the
# XCFramework the blueprint plans rather than with a path into `target/`.
set -euo pipefail
cd "$(dirname "$0")"
ROOT="$(dirname "$(pwd)")"
PROFILE="${1:-release}"
GENERATED="Generated"
LIBRARY="$ROOT/target/$PROFILE/libqh_ffi.dylib"

log() { echo "[build-ffi] $*"; }
die() { echo "[build-ffi] error: $*" >&2; exit 1; }

case "$PROFILE" in
  release|debug) ;;
  *) die "unknown profile '$PROFILE' (expected release or debug)" ;;
esac

log "cargo build -p qh-ffi --profile $PROFILE"
(cd "$ROOT" && cargo build -p qh-ffi --profile "$PROFILE")
[ -f "$LIBRARY" ] || die "cargo reported success but $LIBRARY is missing"

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

log "regenerated $(ls "$GENERATED"/QueryHiveFFI "$GENERATED"/qh_ffiFFI | tr '\n' ' ')"
