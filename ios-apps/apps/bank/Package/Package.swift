// swift-tools-version:5.9
import PackageDescription

// Everything of the bank app that is not a view:
//   bank_coreFFI  the Rust core (crates/bank-core), as a C module
//   BankCore      its generated Swift API (UniFFI)
//   BankKit       settings, keychain, pins, app state: Foundation only, so it also builds on Linux
// `Generated/` is made by `make bindings` (Linux) or `make apple` (macOS), from the ios-apps root.

#if os(Linux)
// For `swift test` on Linux: the header as generated, and the debug build of the crate.
let rustLibs = Context.packageDirectory + "/../../../target/debug"
let ffi: Target = .target(
    name: "bank_coreFFI",
    path: "Generated/bank_coreFFI",
    linkerSettings: [
        .unsafeFlags(["-L", rustLibs, "-Xlinker", "-rpath", "-Xlinker", rustLibs]),
        .linkedLibrary("bank_core"),
    ]
)
#else
// iOS, the simulator and macOS (for `swift test` on a Mac): static libraries + the same header.
let ffi: Target = .binaryTarget(name: "bank_coreFFI", path: "Generated/bank_coreFFI.xcframework")
#endif

let package = Package(
    name: "BankKit",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [.library(name: "BankKit", targets: ["BankKit"])],
    targets: [
        ffi,
        .target(name: "BankCore", dependencies: ["bank_coreFFI"], path: "Generated/BankCore"),
        .target(name: "BankKit", dependencies: ["BankCore"]),
        .testTarget(name: "BankKitTests", dependencies: ["BankKit"]),
    ]
)
