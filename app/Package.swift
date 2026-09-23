// swift-tools-version:5.10
import PackageDescription
import Foundation

// The target is rooted at Sources/TrinoExporter because that is where the sources already
// lived when this app was a half-finished port; the executable it produces is QueryHive, which
// is the name app/build.sh looks for and the name the bundle carries.
//
// `Generated/` is the Rust engine's Swift surface, produced by app/build-ffi.sh and committed
// (blueprint §8, "Binding … di-commit"). UniFFI's generator splits it in two, so the package does
// too: `qh_ffiFFI` is the C module — the header and modulemap the generated Swift calls into —
// and `QueryHiveFFI` is the generated Swift itself. The app imports only the second, and
// `RustEngine` is the one type that does (blueprint §1.6).
//
// The library's location is computed rather than written down, and that is a compromise worth
// naming. `swift build` has no pre-build hook, so the Rust dylib has to exist before the link
// step; a path typed into this file would be a path on one machine, so this one is derived from
// the manifest's own location and prefers `release` over `debug` — a debug Rust library is not
// what an app built `-c release` should be loading, but a `cargo build -p qh-ffi` without
// `--release` should still link instead of failing forty seconds in. When nothing has been built,
// the release path is named anyway so the linker error says which file is missing.
//
// This is the placeholder for the artifact the blueprint plans (`target/ffi/QueryHiveFFI.xcframework`,
// built by an xtask): the crate builds a `cdylib` today, and an XCFramework wants a `staticlib`,
// which is a crate-type line in `crates/qh-ffi/Cargo.toml` — a change that belongs to the FFI and
// not to the app, so it is reported rather than made.
//
// `unsafeFlags` is what tells the linker where that library is, and it is the reason this package
// is a root package rather than something another package can depend on. Nothing depends on it.
let packageDirectory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let repositoryRoot = packageDirectory.deletingLastPathComponent()
let ffiLibraryDirectory: String = {
    let configurations = ["release", "debug"]
    for configuration in configurations {
        let candidate = repositoryRoot
            .appendingPathComponent("target")
            .appendingPathComponent(configuration)
        let library = candidate.appendingPathComponent("libqh_ffi.dylib")
        if FileManager.default.fileExists(atPath: library.path) { return candidate.path }
    }
    return repositoryRoot.appendingPathComponent("target/release").path
}()

let package = Package(
    name: "QueryHive",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "QueryHive", targets: ["QueryHive"])
    ],
    targets: [
        .executableTarget(
            name: "QueryHive",
            dependencies: ["QueryHiveFFI"],
            path: "Sources/TrinoExporter",
            linkerSettings: [
                .unsafeFlags([
                    "-L", ffiLibraryDirectory,
                    "-lqh_ffi",
                    // The library's install name is an absolute path into Cargo's `deps`
                    // directory (a `cdylib` handles its own install name), so this rpath is
                    // belt-and-braces for the day it stops being one.
                    "-Xlinker", "-rpath", "-Xlinker", ffiLibraryDirectory,
                ])
            ]
        ),
        // The two halves of the generated surface, in the names the generator produced. A C target
        // needs its public headers at `publicHeadersPath`, and this one has nothing else in it.
        .target(
            name: "qh_ffiFFI",
            path: "Generated/qh_ffiFFI",
            publicHeadersPath: "."
        ),
        .target(
            name: "QueryHiveFFI",
            dependencies: ["qh_ffiFFI"],
            path: "Generated/QueryHiveFFI"
        ),
        // The logic that had no way to be checked without rendering it and looking
        // (`Event`'s decoding, and the contract `DatabaseEngine` promises) lives here.
        .testTarget(
            name: "QueryHiveTests",
            dependencies: ["QueryHive"],
            path: "Tests/TrinoExporterTests"
        ),
    ]
)
