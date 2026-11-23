# ios-apps

iOS apps for some of the [c4ffein Python cli utils](https://github.com/c4ffein/c4ffein/blob/main/repos/repos-python-cli-utils.md):
one thin SwiftUI shell per app, over one Rust workspace exposed to Swift through [UniFFI](https://github.com/mozilla/uniffi-rs).

First app: **Companion**, a native client for a [companion](https://github.com/c4ffein/companion) server.

## Status

| Part | State |
| :-- | :-- |
| `crates/pinned-http`, `crates/companion-core` (Rust) | Tested on Linux, the client against the real `companion.py` |
| `apps/companion/Package` (Swift, no UI: settings, keychain, app state) | Tested on Linux through the generated bindings, against the real `companion.py`. The keychain code is Apple-only and has not been compiled |
| `apps/companion/App`, `ShareExtension`, `project.yml`, `scripts/build-apple.sh` | **Written without a Mac: syntax-checked only, never compiled or run.** Expect to fix things on the first Xcode build |

## Layout

```
crates/
  pinned-http/       ureq + rustls agent that pins the leaf certificate SHA-256: shared by every app
  companion-core/    the companion API, exported with UniFFI
  uniffi-bindgen/    the bindings generator, pinned to the workspace's uniffi version
apps/companion/
  Package/           Swift package: generated bindings + CompanionKit (everything that is not a view)
  App/               SwiftUI views: Files, Pad, Live, Settings
  ShareExtension/    "Companion" in the share sheet
  project.yml        XcodeGen spec (no .xcodeproj in git)
scripts/             bindings.sh, build-apple.sh, swift-test.sh
```

The rule: a view holds no logic. What can be decided without UIKit is decided in `CompanionKit`
(`AppModel`), where it runs on Linux and is tested; what touches the network or crypto is in Rust.

## Companion: what the app does

- **Files**: the server's files, newest first, refreshed while on screen. Tap to download and preview
  (Quick Look, which also shares and saves). `+` uploads photos, videos or anything from Files, streamed
  from disk. Swipe a file to present it to every client.
- **Pad**: the shared pad, synced both ways: edits are sent when typing pauses, remote edits are taken
  unless there is an unsent local one (last write wins, as in the web UI).
- **Live**: follows what is being presented, and opens each new file.
- **Share sheet**: from any app, files are uploaded; links and selected text are appended to the pad.
- **TLS**: the same model as the CLI. Plain http works as is. https requires a pinned certificate:
  Settings reads the server's certificate, shows its SHA-256 to compare with `companion fingerprint`,
  and pins it. With a self-signed certificate the pin alone is the trust; otherwise chain and hostname
  are verified too. Redirects are never followed.

Not there yet: token rotation, the admin commands, camera capture, setting a server up by QR code.

## Build

Linux or macOS, no Xcode needed:

```sh
make test         # Rust unit tests + TLS pinning tests
make e2e          # Rust client against companion.py   (COMPANION_PY=/path/to/companion.py)
make swift-test   # Swift -> UniFFI -> Rust -> companion.py   (uses Docker when there is no swift)
make check        # fmt, clippy, and all of the above
```

`COMPANION_PY` defaults to `../clones/companion/src/companion.py`.

macOS, with Xcode 16+ and `brew install xcodegen`:

```sh
make xcode        # builds the Rust core as an xcframework, generates and opens Companion.xcodeproj
```

Before running on a device, replace `dev.cafeine` with an identifier of yours in `apps/companion/project.yml`
and in `appGroup` (`Package/Sources/CompanionKit/Settings.swift`), and set your team. The app and its share
extension share the settings through that app group, and the client secret through the keychain.

## Adding an app

1. `crates/<name>-core`: the logic, with `uniffi::setup_scaffolding!()` and `crate-type = ["lib", "staticlib", "cdylib"]`.
   Network access goes through `pinned-http`.
2. `apps/<name>/`: copy the shape of `apps/companion` (`Package/`, `App/`, `project.yml`).
3. `scripts/bindings.sh <name>-core <name>` and `scripts/build-apple.sh <name>-core <name>` take it from there.
