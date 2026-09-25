// This file exists because a SwiftPM clang target with only a header and a modulemap
// produces no object file, yet the linker still expects `<target>.o` whenever another
// target (the executable, or the test bundle that embeds it) links against it. One
// translation unit that includes the generated header gives clang something to emit.
// app/build-ffi.sh rewrites it on every regeneration, so it survives `./build-ffi.sh`.
#include "qh_ffiFFI.h"
