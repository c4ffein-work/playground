#!/bin/sh
# macOS only. Builds a core crate for iOS, the simulator and this Mac, and packs it
# with its C header as an xcframework in the app's Swift package.
#   scripts/build-apple.sh <crate> <app>      e.g. scripts/build-apple.sh companion-core companion
# Apple silicon only: add x86_64-apple-ios / x86_64-apple-darwin (and `lipo` them in) for an Intel Mac.
set -eu
crate=$1 app=$2
root=$(cd "$(dirname "$0")/.." && pwd)
lib=$(echo "$crate" | tr - _)
generated="$root/apps/$app/Package/Generated"
targets="aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin"

[ "$(uname)" = Darwin ] || { echo "build-apple.sh needs macOS and Xcode" >&2; exit 1; }
cd "$root"
"$root/scripts/bindings.sh" "$crate" "$app"

export IPHONEOS_DEPLOYMENT_TARGET=17.0 MACOSX_DEPLOYMENT_TARGET=14.0
set --
for target in $targets; do
    rustup target add "$target" >/dev/null
    cargo build --release -p "$crate" --target "$target"
    set -- "$@" -library "target/$target/release/lib$lib.a" -headers "$generated/${lib}FFI/include"
done

rm -rf "$generated/${lib}FFI.xcframework"
xcodebuild -create-xcframework "$@" -output "$generated/${lib}FFI.xcframework"
