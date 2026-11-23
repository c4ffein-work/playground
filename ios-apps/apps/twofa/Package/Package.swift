// swift-tools-version:5.9
import PackageDescription

// Everything of the 2fa app that is not a view:
//   twofa_coreFFI  the Rust core (crates/twofa-core), as a C module
//   TwofaCore      its generated Swift API (UniFFI)
//   TwofaKit       the key store and the app state: Foundation only, so it also builds on Linux
// `Generated/` is made by `make bindings` (Linux) or `make apple` (macOS), from the ios-apps root.

#if os(Linux)
// For `swift test` on Linux: the header as generated, and the debug build of the crate.
let rustLibs = Context.packageDirectory + "/../../../target/debug"
let ffi: Target = .target(
    name: "twofa_coreFFI",
    path: "Generated/twofa_coreFFI",
    linkerSettings: [
        .unsafeFlags(["-L", rustLibs, "-Xlinker", "-rpath", "-Xlinker", rustLibs]),
        .linkedLibrary("twofa_core"),
    ]
)
#else
// iOS, the simulator and macOS (for `swift test` on a Mac): static libraries + the same header.
let ffi: Target = .binaryTarget(name: "twofa_coreFFI", path: "Generated/twofa_coreFFI.xcframework")
#endif

let package = Package(
    name: "TwofaKit",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [.library(name: "TwofaKit", targets: ["TwofaKit"])],
    targets: [
        ffi,
        .target(name: "TwofaCore", dependencies: ["twofa_coreFFI"], path: "Generated/TwofaCore"),
        .target(name: "TwofaKit", dependencies: ["TwofaCore"]),
        .testTarget(name: "TwofaKitTests", dependencies: ["TwofaKit"]),
    ]
)
