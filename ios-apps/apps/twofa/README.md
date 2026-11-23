# 2fa

An authenticator: the two-factor codes (TOTP and HOTP) of your accounts, on the phone.
A port of the [2fa](https://github.com/c4ffein/2fa) command line, itself interoperable with
[rsc/2fa](https://github.com/rsc/2fa): the app reads and writes the same keychain file (`~/.2fa`),
so keys move between the phone and the terminal, counters included.

Directories and identifiers say `twofa` (an identifier cannot start with a digit); the app is called 2fa.

## Status

| Part | State |
| :-- | :-- |
| `crates/twofa-core` (Rust) | Tested on Linux: RFC 4226 and RFC 6238 vectors, and the real `2fa.py` in both directions |
| `Package` (`TwofaKit`: key store, app state) | Tested on Linux through the generated bindings. `Keychain.swift` and `Device.swift` (keychain, Face ID, clipboard) are Apple-only and have not been compiled |
| `App`, `project.yml` | **Written without a Mac: syntax-checked only, never compiled or run.** Expect to fix things on the first Xcode build |

## What the app does

- **Codes**: the keys, sorted by name as `2fa` lists them, each with its current code and one countdown
  for all (every key has the same 30 seconds). Tap a row to copy its code.
- **Add**: scan the QR code a service shows (an `otpauth://` URI), paste that link, or type the key.
  A scanned key is shown for review, with a name to edit, before it is stored.
- **Counter-based keys (HOTP)**: no code until asked; the button gives the next one.
- **Rename, delete**: swipe a row.
- **Import, export**: the `2fa` keychain file. Import merges: new names are added, a key already there is
  never written over, a counter only moves forward. Into an empty app, the file is taken byte for byte,
  and exported the same, lines that are not keys included.
- **Lock**: optionally, Face ID (or the passcode) each time the app comes back on screen.

Codes are computed exactly as `2fa` does: HMAC-SHA-1, 30 second steps, 6, 7 or 8 digits. A counter-based key
stores the last counter used, and its first code is made from counter 1, not 0, as with `2fa -add -hotp`.

### Refused, rather than given wrong codes

The file format has a name, a size, a key and a counter: nothing else. So an `otpauth://` URI is refused,
with the reason, when it asks for `algorithm` other than SHA1, a `period` other than 30, `digits` other
than 6, 7 or 8, a type other than `totp` or `hotp`, or an `encoder` (Steam). So is a Google Authenticator
export (`otpauth-migration://`, several keys in one QR code): scan each service's own QR code instead.

Names are what `2fa` accepts on its command line (letters, digits, `-`, `/`, `=`), so that a key added on
the phone can be asked for in the terminal. The name suggested after a scan is the issuer and the account
with everything else turned into `-`. An imported file may hold other names: they are kept and shown as they are.

## Security model

- **Where the secrets are.** In the iOS keychain, as one generic password holding the whole keychain file,
  `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`: readable only while the phone is unlocked, not synced to
  iCloud Keychain, and not restored on another device, even from a backup of this one. Nothing secret is in
  `UserDefaults` (only whether the lock is on) or in a file.
- **One item, not one per key.** The file is what the core works on and what is imported and exported, so
  nothing is converted and there is no list of names to keep in step with the secrets. Every change, a
  counter going up included, replaces that single item in one `SecItemUpdate`: it is the new file or still
  the old one. A read that fails (as opposed to finding nothing) stops the change: it is never taken for
  an empty keychain and written over.
- **Counters.** The new counter is stored before the code is shown. If storing fails, no code is shown:
  a code shown but not counted would be shown again, and refused.
- **What leaves the device.** Nothing, by itself: the app has no network code at all (the core does not link
  an HTTP client, and the app declares no network use). Two things leave, when you ask: a code you copy,
  and the file you export.
- **Clipboard.** A copied code is marked local-only (no Universal Clipboard to your other devices) and
  expires after 60 seconds. Until then any app you paste into, or that reads the clipboard, can see it.
- **Export.** The file is not encrypted: it is the format of `~/.2fa`, every secret in clear. The app says so
  before writing it. Where it goes then (Files, iCloud Drive, AirDrop) is yours to choose.
- **No backup but export.** Because the keychain item stays on this device, a lost or replaced phone loses
  the keys. Export the file, or keep each service's recovery codes.
- **The lock is a screen, not encryption.** Face ID decides whether the app shows and does anything; while
  locked it holds no code in memory, and the app switcher sees a blank screen. The secrets are protected by
  the keychain and the phone's passcode either way, not by the lock: it does not stop someone who can
  already read this app's keychain on an unlocked, compromised phone.
- **QR codes.** The camera image is read on the device, and only for a QR code; what it says is parsed in
  Rust and never logged.

## Tested

```sh
cargo test -p twofa-core                                           # 15 tests
TWOFA_PY=/path/to/2fa.py cargo test -p twofa-core -- --ignored     # 2 tests, against the real 2fa.py
scripts/bindings.sh twofa-core twofa && apps/twofa/swift-test.sh   # 14 tests (Docker when there is no swift)
```

- Rust: HOTP against RFC 4226 appendix D, TOTP against the SHA-1 rows of RFC 6238 appendix B and against
  `2fa`'s own test values; base32 as Python decodes it; the file read line by line as `2fa.py` reads it
  (what it ignores is ignored, and reported with a line number); add, remove, rename, counters, merge;
  `otpauth://` URIs, with each refusal and its message.
- Against `2fa.py` (`cross_check.rs`): a file written by the core is read by `2fa.py` with the same codes for
  6, 7 and 8 digits, and the same file bytes after each counter step; a file written by `2fa.py -add` is
  the one the core writes for the same keys; the two take turns on one counter.
- Swift (`TwofaKitTests`), with the clock, the clipboard, the secret store and Face ID replaced: codes follow
  the clock and are not recomputed within a step; a failed write leaves the file and the shown code as they
  were; a failed read is not an empty keychain; copy, import, export, drafts from QR codes, the lock.

`make e2e` and `make check` run the ignored tests of every crate: they need `TWOFA_PY` set, beside `COMPANION_PY`.

## Not tested

- Everything in `App/`, and `project.yml`: never built. The views hold no decision, but SwiftUI details
  (sheets and alerts in sequence, the swipe actions, the file importer on a file with no extension) are
  to be checked by hand.
- `KeychainSecretStore` against a real keychain: that an update is atomic there is Apple's contract, not
  something these tests show. `DeviceAuthenticator` and `SystemClipboard` likewise.
- The scanner: the camera, the permission question, real QR codes.
- Clock errors: like every TOTP app, the phone's clock must be right to within the server's tolerance.
