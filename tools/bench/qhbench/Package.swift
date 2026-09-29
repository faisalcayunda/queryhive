// swift-tools-version:5.9
// Black-box benchmark harness. It is not linked into the app and depends on Apple frameworks only.
import PackageDescription

let package = Package(
    name: "qhbench",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "qhbench",
            path: "Sources/qhbench",
            linkerSettings: [
                .linkedFramework("ScreenCaptureKit"),
                .linkedFramework("ApplicationServices"),
                .linkedFramework("CoreGraphics"),
                .linkedFramework("CoreMedia"),
                .linkedFramework("AppKit"),
            ]
        )
    ]
)
