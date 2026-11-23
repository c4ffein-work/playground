# Bank

An iOS client for the Qonto API: a port of the network side of [bank](https://github.com/c4ffein/bank) (`bank.py`),
for the one thing a phone is better at than a terminal. See the accounts and the transactions, find the ones that
still miss a receipt, and attach it on the spot: a photo, a picture from the library, a PDF from Files.

## Status

| Part | State |
| :-- | :-- |
| `crates/bank-core` (Rust) | Tested on Linux against a mock of the API, over http and over pinned https. **Never run against the real API** |
| `Package` (Swift, no UI: settings, pins, app state) | Tested on Linux through the generated bindings, against the same mock. The keychain code is Apple-only and has not been compiled |
| `App`, `project.yml` | **Written without a Mac: syntax-checked only, never compiled or run.** Expect to fix things on the first Xcode build |

The mock (`crates/bank-core/tests/mock/mod.rs`) is derived from what `bank.py` sends and what its `test.py` expects,
**not from Qonto's documentation**. The first run against the real API may surface differences. Where the CLI does
not say, it is a guess, and the parser treats it as optional: the fields of a bank account besides `id` (name, IBAN,
`balance_cents`...: the CLI prints `/v2/organization` as it comes), and of an attachment besides `id` and `url`.

## What the app does

- **Transactions** of the selected account, newest first, a page of 100 at a time as the list is scrolled.
  A switch keeps only the ones **missing a receipt** (`bank transactions no-invoice`); a date filter takes this year,
  last year, or any days (`date=2024`, `date>=`, `date<=`, finer).
- **A transaction**: amount (and the amount in the foreign currency it was paid in), dates, counterparty, reference,
  note, and its receipts. Tap one to download and preview it (Quick Look, which also shares and saves).
- **Attach a receipt**: camera, photo library or Files, with upload progress. PDF, JPEG and PNG, recognised by their
  first bytes as the CLI does; a photo in another format (HEIC) is converted to JPEG first. A file the transaction
  already has is not sent again: the app says so and offers "Send anyway" (`bank justify ... force`).
- **Accounts** with their balances; tap one to list its transactions.
- **Settings**: the pinned certificates, the API key, clearing downloads, signing out.

Left out, on purpose: `bank link`, `bank unlinked` and `bank reconcile`. They compare the bank with an archive of
invoices on a disk, which a phone does not have. The local state that goes with them is not kept either: no
`justify_log.jsonl`, no links file. A receipt sent from the phone is "on Qonto only" for `bank reconcile` until it
is also in the archive.

## Security model

- **Where the key goes.** The API base URL is a constant (`https://thirdparty.qonto.com`), not a setting. The
  constructor that takes another one exists for the tests and refuses anything but a loopback address. Redirects
  are never followed. The `Authorization` header is only sent to the API: attachment downloads go to pre-signed
  storage URLs without it, and only over https.
- **Pinned TLS, twice.** No request without a pinned certificate: the SHA-256 of the leaf certificate is checked
  during the handshake, on top of chain and hostname verification against the bundled Mozilla roots
  (`pinned_http`, `Trust::PinnedWithCa`). The API and the attachments storage (S3) each have their own list of pins,
  as `certificates.qonto` and `certificates.aws_s3` in the CLI. A list, because certificates rotate: the next one
  can be pinned before the current one is gone.
- **How pins get onto the phone** (`Package/Sources/BankKit/Pins.swift`). The CLI runs `tls-pin-get`; a phone runs
  no command, so:
  1. *Read it*: Settings connects to the host, sends nothing, and shows the SHA-256 it is served. A certificate that
     does not verify against the public CAs for that name cannot be pinned at all. One that does is shown to be
     compared with what `tls-pin-get thirdparty.qonto.com:443` prints on a trusted computer, then pinned by a tap.
     Skipping the comparison makes this trust-on-first-use backed by the public CAs: what a browser gives, no less.
  2. *Paste them*: the output of `tls-pin-get`, one SHA-256 per line, validated as strictly as the CLI does.

  When a certificate changes, requests fail with a pin mismatch, nothing is sent, and Settings shows what was served
  and asks for the same steps again. Nothing is ever pinned without the user.
- **The key** (organization slug and secret key) is in the keychain, `WhenUnlockedThisDeviceOnly`: not in backups,
  not on another device. The pins and the selected account are in `UserDefaults`: not secret.
- **Files.** Downloaded attachments go to the caches directory, mode 0600, under names made from the attachment id
  (never from a name the API gives). `project.yml` asks for complete data protection (encrypted while locked).
  "Sign out" removes the key and the downloads.
- **What the API answers is not trusted**: ids that go in a path are validated as UUIDs, error bodies are cut and
  cleaned before they are shown, a `next_page` that is not a later page is an error.
- **Not there**: no Face ID gate on opening the app, no blur in the app switcher, no jailbreak detection.

## Sending a receipt once

As `bank justify`, minus its local log:

1. Unless forced, Qonto is asked what the transaction has. Each attachment is downloaded (once: they are kept by id)
   and hashed; if one is the file, nothing is sent.
2. If that check cannot be done (no pin for the storage, a download fails), nothing is sent, and "Send anyway" is offered.
3. The upload carries `X-Qonto-Idempotency-Key`: the same UUIDv5 as the CLI for the same file and transaction, so a
   retry after a lost answer, from the phone or from the desktop, is one attachment. "Send anyway" uses a random key.

The CLI also remembers in `justify_log.jsonl` what it sent, and will not resend a file that was later removed on
Qonto. The phone only knows what Qonto has now.

## Money

Amounts are the integers of cents the API gives (`amount_cents`, `local_amount_cents`, `balance_cents`), `i64` in
Rust and `Int64` in Swift, never a float: the `amount` and `balance` floats of the API are ignored, and a float in
place of an integer is refused. The sign comes from `side`. Text is made at the very end, from an exact `Decimal`
(cents × 10⁻²), in the user's locale.

`bank.py` does the same integer arithmetic (`money // 100`, `money % 100`). Two differences: it prints `?` for any
currency but EUR and USD, where the app uses the currency code; and like it, the app assumes two decimals for every
currency, which is what "cents" says but would be wrong for a local amount in yen if Qonto reports those in units.

## Other differences with the CLI

- The CLI only uses the first bank account. The app lists them and remembers the one that was chosen.
- The CLI filters by year; the app also by day. Both mean UTC days (`…T00:00:00Z` to `…T23:59:59Z`).
- A picked photo that is already a JPEG or a PNG is sent byte for byte, as the CLI sends a file: its metadata
  (location, when the photo picker is set to include it) goes with it. A photo taken in the app, or converted from
  HEIC, is re-encoded and carries none.
- The API host is not configurable, and the attachments host is only used to read its certificate: a download goes
  where the API says, and the pin decides.

## Tests

From `ios-apps/`:

```sh
cargo test -p bank-core      # unit tests, the mock over http (tests/api.rs), the mock over pinned https (tests/tls.rs)
apps/bank/swift-test.sh      # Swift -> UniFFI -> Rust -> the same mock, started for the occasion
```

Covered: the requests byte for byte as the CLI makes them (paths, query order, `Authorization`, empty `User-Agent`,
multipart body, idempotency key against Python's `uuid5`), pagination, both filters, error messages (`test.py`'s
cases), downloads (streamed, hashed, complete or absent, no credentials sent), every branch of sending a receipt,
pin mismatch for each host, chain and hostname verification, redirects; in Swift, the settings and keychain split,
both ways of adding pins, money formatting, and the app model end to end.

Not covered: the real API, the views, the keychain, the camera and pickers, HEIC conversion.

## Build

```sh
make xcode APP=bank    # macOS, Xcode 16+, xcodegen: builds the Rust core, generates and opens Bank.xcodeproj
```

Before running on a device, replace `dev.cafeine` with an identifier of yours in `project.yml` and set your team.
