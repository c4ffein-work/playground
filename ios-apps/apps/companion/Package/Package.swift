// swift-tools-version:5.9
import PackageDescription

// Everything of the companion app that is not a view:
//   companion_coreFFI  the Rust core (crates/companion-core), as a C module
//   CompanionCore      its generated Swift API (UniFFI)
//   CompanionKit       settings, keychain, async wrappers: Foundation only, so it also builds on Linux
// `Generated/` is made by `make bindings` (Linux) or `make apple` (macOS), from the ios-apps root.

#if os(Linux)
// For `swift test` on Linux: the header as generated, and the debug build of the crate.
let rustLibs = Context.packageDirectory + "/../../../target/debug"
let ffi: Target = .target(
    name: "companion_coreFFI",
    path: "Generated/companion_coreFFI",
    linkerSettings: [
        .unsafeFlags(["-L", rustLibs, "-Xlinker", "-rpath", "-Xlinker", rustLibs]),
        .linkedLibrary("companion_core"),
    ]
)
#else
// iOS, the simulator and macOS (for `swift test` on a Mac): static libraries + the same header.
let ffi: Target = .binaryTarget(name: "companion_coreFFI", path: "Generated/companion_coreFFI.xcframework")
#endif

let package = Package(
    name: "CompanionKit",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [.library(name: "CompanionKit", targets: ["CompanionKit"])],
    targets: [
        ffi,
        .target(name: "CompanionCore", dependencies: ["companion_coreFFI"], path: "Generated/CompanionCore"),
        .target(name: "CompanionKit", dependencies: ["CompanionCore"]),
        .testTarget(name: "CompanionKitTests", dependencies: ["CompanionKit"]),
    ]
)
