// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "Yeet",
    platforms: [
        .macOS(.v15)
    ],
    products: [
        .library(name: "HarnessCallCore", targets: ["HarnessCallCore"]),
        .executable(name: "yeet", targets: ["Yeet"]),
    ],
    dependencies: [
        .package(
            url: "https://github.com/SwiftTUI/swift-tui",
            .upToNextMinor(from: "0.9.11")
        )
    ],
    targets: [
        .target(
            name: "HarnessCallCore",
            resources: [
                .copy("Resources/Runtime")
            ]
        ),
        .executableTarget(
            name: "Yeet",
            dependencies: [
                "HarnessCallCore",
                .product(name: "SwiftTUI", package: "swift-tui")
            ]
        ),
        .testTarget(
            name: "HarnessCallCoreTests",
            dependencies: ["HarnessCallCore"],
            resources: [
                .copy("Fixtures")
            ]
        ),
        .testTarget(
            name: "YeetTests",
            dependencies: [
                "Yeet",
                .product(name: "SwiftTUI", package: "swift-tui"),
                .product(name: "SwiftTUIRuntime", package: "swift-tui"),
                .product(name: "SwiftTUIViews", package: "swift-tui")
            ]
        )
    ]
)
