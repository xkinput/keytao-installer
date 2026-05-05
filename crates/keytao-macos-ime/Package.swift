// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "KeyTaoIME",
    platforms: [.macOS(.v12)],
    targets: [
        // Wrapper target that exposes the pre-built C dylib to Swift
        .systemLibrary(
            name: "CKeytaoCore",
            path: "Sources/CKeytaoCore",
            pkgConfig: nil,
            providers: nil
        ),
        .executableTarget(
            name: "KeyTaoIME",
            dependencies: ["CKeytaoCore"],
            path: "Sources/KeyTaoIME",
            resources: [.copy("Resources")]
        ),
    ]
)
