// swift-tools-version:5.9
import PackageDescription

// Everything of the pass app that is not a view:
//   pass_coreFFI  the Rust core (crates/pass-core), as a C module
//   PassCore      its generated Swift API (UniFFI)
//   PassKit       settings, keychain, app state: Foundation only, so it also builds on Linux
// `Generated/` is made by `make bindings` (Linux) or `make apple` (macOS), from the ios-apps root.

#if os(Linux)
// For `swift test` on Linux: the header as generated, and the debug build of the crate.
let rustLibs = Context.packageDirectory + "/../../../target/debug"
let ffi: Target = .target(
    name: "pass_coreFFI",
    path: "Generated/pass_coreFFI",
    linkerSettings: [
        .unsafeFlags(["-L", rustLibs, "-Xlinker", "-rpath", "-Xlinker", rustLibs]),
        .linkedLibrary("pass_core"),
    ]
)
#else
// iOS, the simulator and macOS (for `swift test` on a Mac): static libraries + the same header.
let ffi: Target = .binaryTarget(name: "pass_coreFFI", path: "Generated/pass_coreFFI.xcframework")
#endif

let package = Package(
    name: "PassKit",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [.library(name: "PassKit", targets: ["PassKit"])],
    targets: [
        ffi,
        .target(name: "PassCore", dependencies: ["pass_coreFFI"], path: "Generated/PassCore"),
        .target(name: "PassKit", dependencies: ["PassCore"]),
        .testTarget(name: "PassKitTests", dependencies: ["PassKit"]),
    ]
)
