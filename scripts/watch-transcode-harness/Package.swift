// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "watch-transcode-harness",
    platforms: [.macOS(.v15)],
    dependencies: [.package(path: "../../FlaccyCore")],
    targets: [
        .executableTarget(
            name: "harness",
            dependencies: [.product(name: "FlaccyCore", package: "FlaccyCore")],
            swiftSettings: [.swiftLanguageMode(.v5), .defaultIsolation(MainActor.self)]
        )
    ]
)
