#!/bin/sh
# Generates the Swift bindings of a core crate into an app's Swift package.
#   scripts/bindings.sh <crate> <app>      e.g. scripts/bindings.sh companion-core companion
# Output, in apps/<app>/Package/Generated/:
#   <Module>/<crate>.swift             the Swift API (target `CompanionCore`, ...)
#   <crate>FFI/include/                the C header + module map (used as is on Linux,
#                                      copied into the xcframework by build-apple.sh)
set -eu
crate=$1 app=$2
root=$(cd "$(dirname "$0")/.." && pwd)
lib=$(echo "$crate" | tr - _)                      # companion-core -> companion_core
module=$(echo "$crate" | awk -F- '{for (i = 1; i <= NF; i++) printf "%s%s", toupper(substr($i, 1, 1)), substr($i, 2)}')
out="$root/apps/$app/Package/Generated"

cd "$root"
cargo build -p "$crate"
case "$(uname)" in Darwin) dylib="target/debug/lib$lib.dylib" ;; *) dylib="target/debug/lib$lib.so" ;; esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cargo run -q -p uniffi-bindgen -- generate --library "$dylib" --language swift --no-format --out-dir "$tmp"

rm -rf "$out/$module" "$out/${lib}FFI"
mkdir -p "$out/$module" "$out/${lib}FFI/include"
cp "$tmp/$lib.swift" "$out/$module/"
cp "$tmp/${lib}FFI.h" "$out/${lib}FFI/include/"
cp "$tmp/${lib}FFI.modulemap" "$out/${lib}FFI/include/module.modulemap"
: > "$out/${lib}FFI/shim.c"                        # SwiftPM wants a source file in a C target
echo "bindings: $out"
