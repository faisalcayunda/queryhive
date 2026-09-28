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
// naming. `swift build` has no pre-build hook, so the Rust artifact has to exist before the link
// step; a path typed into this file would be a path on one machine, so this one is derived from
// the manifest's own location and prefers `release` over `debug` -- a debug Rust library is not
// what an app built `-c release` should be loading, but a `cargo build -p qh-ffi` without
// `--release` should still link instead of failing forty seconds in. When nothing has been built,
// the release path is named anyway so the linker error says which file is missing.
//
// What is linked is the `staticlib`, not the `cdylib` (`app/build-ffi.sh` stages the archive into
// `target/ffi/static/<profile>/`). That is the whole reason the bundle is relocatable: a `cdylib`
// carries an absolute install name pointing into Cargo's output directory, so an app that linked
// it ran on this machine and nowhere else, and shipping it would have meant shipping a second
// file plus a path to find it. An archive is copied into the app's own Mach-O instead, so
// `dist/QueryHive.app` is self-contained.
//
// The price of an archive is that it cannot record its own dependencies, so
// `staticFrameworks` below names them by hand. They are what `cargo rustc -p qh-ffi --release
// --lib -- --print native-static-libs` prints; if a dependency ever starts using another system
// library, the link fails with an undefined symbol here rather than silently at runtime.
//
// `unsafeFlags` is what tells the linker where that library is, and it is the reason this package
// is a root package rather than something another package can depend on. Nothing depends on it.
let packageDirectory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
let repositoryRoot = packageDirectory.deletingLastPathComponent()
let ffiLibraryDirectory: String = {
    for configuration in ["release", "debug"] {
        let candidate = repositoryRoot
            .appendingPathComponent("target/ffi/static")
            .appendingPathComponent(configuration)
        let archive = candidate.appendingPathComponent("libqh_ffi.a")
        if FileManager.default.fileExists(atPath: archive.path) { return candidate.path }
    }
    return repositoryRoot.appendingPathComponent("target/ffi/static/release").path
}()

// `-framework` needs a following argument, so each name is written as its own `-Xlinker` pair.
// `.linkedFramework` would read better and is deliberately not used: it does not exist in this
// tools version, and the flag spelling is stable.
let staticFrameworks = ["Security", "CoreFoundation", "SystemConfiguration"]
    .flatMap { ["-Xlinker", "-framework", "-Xlinker", $0] }

let package = Package(
    name: "QueryHive",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "QueryHive", targets: ["QueryHive"])
    ],
    // The one dependency that is not this repository: Sparkle, for updates that install
    // themselves. It is a `binaryTarget` inside Sparkle's own manifest, which means SwiftPM
    // downloads a prebuilt Sparkle.framework from its GitHub release rather than compiling it —
    // and that means a build machine with no network cannot build this app until the archive is
    // vendored. `app/build.sh` is what puts the framework into the bundle; linking it alone
    // produces an app that dies at launch with a library that is nowhere to be found.
    dependencies: [
        .package(url: "https://github.com/sparkle-project/Sparkle", from: "2.10.0")
    ],
    targets: [
        .executableTarget(
            name: "QueryHive",
            dependencies: ["QueryHiveFFI", .product(name: "Sparkle", package: "Sparkle")],
            path: "Sources/TrinoExporter",
            linkerSettings: [
                .unsafeFlags(
                    [
                        "-L", ffiLibraryDirectory,
                        "-lqh_ffi",
                    ]
                        + staticFrameworks
                        + [
                            // Belt-and-braces, and now a no-op: the archive brought its code
                            // with it, so nothing is resolved at load time. Kept because a
                            // `-L` that holds only an archive is a choice this manifest makes
                            // and a future reader deserves to see it was made on purpose.
                            "-Xlinker", "-rpath", "-Xlinker", ffiLibraryDirectory,
                            // Sparkle is the opposite: a real dynamic framework, linked by
                            // SwiftPM from wherever it unpacked the binary, and found at run
                            // time by this rpath. Without it the app links and then dies on
                            // launch with "Library not loaded: @rpath/Sparkle.framework/…",
                            // because `build.sh` puts the framework in the bundle's Frameworks
                            // directory and nothing would tell the loader to look there.
                            "-Xlinker", "-rpath", "-Xlinker", "@loader_path/../Frameworks",
                            "-Xlinker", "-rpath", "-Xlinker", "@executable_path/../Frameworks",
                        ]
                )
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
