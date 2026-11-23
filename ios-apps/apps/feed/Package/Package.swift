// swift-tools-version:5.9
import PackageDescription

// Everything of the feed app that is not a view:
//   feed_coreFFI  the Rust core (crates/feed-core), as a C module
//   FeedCore      its generated Swift API (UniFFI)
//   FeedKit       the feeds, their cookies, the cache, read/unread, app state: Foundation only, so it also builds on Linux
// `Generated/` is made by `make bindings` (Linux) or `make apple` (macOS), from the ios-apps root.

#if os(Linux)
// For `swift test` on Linux: the header as generated, and the debug build of the crate.
let rustLibs = Context.packageDirectory + "/../../../target/debug"
let ffi: Target = .target(
    name: "feed_coreFFI",
    path: "Generated/feed_coreFFI",
    linkerSettings: [
        .unsafeFlags(["-L", rustLibs, "-Xlinker", "-rpath", "-Xlinker", rustLibs]),
        .linkedLibrary("feed_core"),
    ]
)
#else
// iOS, the simulator and macOS (for `swift test` on a Mac): static libraries + the same header.
let ffi: Target = .binaryTarget(name: "feed_coreFFI", path: "Generated/feed_coreFFI.xcframework")
#endif

let package = Package(
    name: "FeedKit",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [.library(name: "FeedKit", targets: ["FeedKit"])],
    targets: [
        ffi,
        .target(name: "FeedCore", dependencies: ["feed_coreFFI"], path: "Generated/FeedCore"),
        .target(name: "FeedKit", dependencies: ["FeedCore"]),
        .testTarget(name: "FeedKitTests", dependencies: ["FeedKit", "FeedCore"]),
    ]
)
