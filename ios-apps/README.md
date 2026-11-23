# ios-apps

iOS apps for some of the [c4ffein Python cli utils](https://github.com/c4ffein/c4ffein/blob/main/repos/repos-python-cli-utils.md):
one thin SwiftUI shell per app, over one Rust workspace exposed to Swift through [UniFFI](https://github.com/mozilla/uniffi-rs).

| App | Port of | |
| :-- | :-- | :-- |
| [Companion](apps/companion) | [companion](https://github.com/c4ffein/companion) | files, pad and presentation of a companion server; share sheet |
| [bank](apps/bank/README.md) | [bank](https://github.com/c4ffein/bank) | Qonto accounts and transactions, find the ones with no receipt and attach one. Tested against a mock only |
| [feed](apps/feed/README.md) | [feed](https://github.com/c4ffein/feed) | RSS/Atom reader: offline cache, read state, articles rewritten from an allowlist before display |
| [pass](apps/pass/README.md) | [pass](https://github.com/c4ffein/pass) | password-store without gpg or git: decrypt, edit, see who can read what; devices trade signed, encrypted bundles through a [drop zone](apps/pass/dropzone/README.md) ([protocol](apps/pass/SYNC.md)) |
| [2fa](apps/twofa/README.md) | [2fa](https://github.com/c4ffein/2fa) | TOTP/HOTP codes, same keychain file as the CLI, QR import |

Each app's README says what it does and what was tested. The sections below describe the first one, Companion.

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
  swift-test.sh      the package tests, against a real server
scripts/             bindings.sh, build-apple.sh, swift.sh (swift, in Docker when there is none)
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

An app `<name>` is `crates/<name>-core` plus `apps/<name>/`: the Makefile finds it by that naming.

1. `crates/<name>-core`: the logic, with `uniffi::setup_scaffolding!()` and `crate-type = ["lib", "staticlib", "cdylib"]`.
   Network access goes through `pinned-http`: `agent` for an API with a known certificate, `web_agent` for arbitrary sites.
   Tests that need something outside the repo are `#[ignore]`d and run by `make e2e`.
2. `apps/<name>/`: the shape of `apps/companion`: `Package/` (bindings + everything that is not a view, tested on Linux),
   `App/` (views only), `project.yml`, and a `swift-test.sh` that runs the package tests through `scripts/swift.sh`.
3. `apps/<name>/README.md`: what the app does and what state it is in.
