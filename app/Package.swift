// swift-tools-version:5.10
import PackageDescription

// The target is rooted at Sources/TrinoExporter because that is where the sources already
// lived when this app was a half-finished port; the executable it produces is QueryHive, which
// is the name app/build.sh looks for and the name the bundle carries.
let package = Package(
    name: "QueryHive",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "QueryHive", targets: ["QueryHive"])
    ],
    targets: [
        .executableTarget(
            name: "QueryHive",
            path: "Sources/TrinoExporter"
        )
    ]
)
