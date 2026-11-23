import Foundation
import XCTest
@testable import TwofaKit

/// The secret of the RFC 4226 and 6238 test vectors, in base32.
private let rfcKey = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
/// The key of 2fa's own tests.
private let hello = "JBSWY3DPEHPK3PXP"
/// An instant of RFC 6238: its 8 digit code is 07081804, and the next step starts one second later.
private let rfcTime: TimeInterval = 1_111_111_109

struct Refused: Error {}

/// A secret store that can be told to fail, as a keychain does while the phone is locked.
final class FlakySecretStore: SecretStore, @unchecked Sendable {
    let inner = InMemorySecretStore()
    var failsReads = false
    var failsWrites = false
    private(set) var writes = 0

    func read(_ account: String) throws -> String? {
        if failsReads { throw Refused() }
        return try inner.read(account)
    }

    func write(_ secret: String?, for account: String) throws {
        if failsWrites { throw Refused() }
        writes += 1
        try inner.write(secret, for: account)
    }
}

@MainActor
final class FakeClipboard: Clipboard {
    private(set) var copies: [(text: String, expiresAfter: TimeInterval)] = []

    nonisolated init() {}

    func copy(_ text: String, expiresAfter: TimeInterval) {
        copies.append((text, expiresAfter))
    }
}

final class FakeAuthenticator: Authenticator, @unchecked Sendable {
    var answer = true
    private(set) var reasons: [String] = []

    func authenticate(reason: String) async -> Bool {
        reasons.append(reason)
        return answer
    }
}

final class KeyStoreTests: XCTestCase {
    func testTheFileIsOneSecretReplacedWhole() throws {
        let secrets = FlakySecretStore()
        let store = KeyStore(secrets: secrets)
        XCTAssertEqual(try store.text(), "")

        let result = try store.update { text in (text + "a 6 \(hello)\n", 42) }
        XCTAssertEqual(result, 42)
        XCTAssertEqual(try store.text(), "a 6 \(hello)\n")
        XCTAssertEqual(try secrets.inner.read("keychain-file"), "a 6 \(hello)\n")
        XCTAssertEqual(secrets.writes, 1)

        // Nothing changed: nothing written.
        try store.update { text in (text, ()) }
        XCTAssertEqual(secrets.writes, 1)

        // No key left: no secret left.
        try store.update { _ in ("", ()) }
        XCTAssertNil(try secrets.inner.read("keychain-file"))
    }

    func testAFailedChangeLeavesTheFileAsItWas() throws {
        let secrets = FlakySecretStore()
        let store = KeyStore(secrets: secrets)
        try store.update { _ in ("a 6 \(hello)\n", ()) }

        XCTAssertThrowsError(try store.update { _ -> (text: String, result: Void) in throw Refused() })
        secrets.failsWrites = true
        XCTAssertThrowsError(try store.update { _ in ("", ()) })
        secrets.failsWrites = false
        XCTAssertEqual(try store.text(), "a 6 \(hello)\n")
    }

    func testAFailedReadIsNotAnEmptyKeychain() throws {
        let secrets = FlakySecretStore()
        let store = KeyStore(secrets: secrets)
        try store.update { _ in ("a 6 \(hello)\n", ()) }

        // If "cannot read" were "no keys", adding one now would write over all the others.
        secrets.failsReads = true
        XCTAssertThrowsError(try store.text())
        XCTAssertThrowsError(try store.update { text in (text + "b 6 \(hello)\n", ()) })
        secrets.failsReads = false
        XCTAssertEqual(try store.text(), "a 6 \(hello)\n")
        XCTAssertEqual(secrets.writes, 1)
    }
}

/// Every test is `async` and on the main actor, one by one: a `@MainActor` test class does not run on Linux.
final class AppModelTests: XCTestCase {
    private var secrets: FlakySecretStore!
    private var store: KeyStore!
    private var defaults: UserDefaults!
    private var suite: String!
    private var clipboard: FakeClipboard!
    private var authenticator: FakeAuthenticator!
    /// The time the model is told it is.
    private var time = rfcTime

    override func setUp() async throws {
        secrets = FlakySecretStore()
        store = KeyStore(secrets: secrets)
        suite = "twofa-tests-\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suite)!
        clipboard = FakeClipboard()
        authenticator = FakeAuthenticator()
        time = rfcTime
    }

    override func tearDown() async throws {
        defaults.removePersistentDomain(forName: suite)
    }

    @MainActor private func model() -> AppModel {
        AppModel(store: store, defaults: defaults, clipboard: clipboard, authenticator: authenticator) { [unowned self] in
            Date(timeIntervalSince1970: self.time)
        }
    }

    private func draft(_ name: String, _ secret: String, digits: Int = 6, counter: UInt64? = nil) -> KeyDraft {
        var draft = KeyDraft()
        draft.name = name
        draft.secret = secret
        draft.digits = digits
        draft.isCounterBased = counter != nil
        draft.counter = counter ?? 0
        return draft
    }

    @MainActor private func code(_ model: AppModel, _ name: String) -> String? {
        model.keys.first { $0.name == name }?.code
    }

    @MainActor func testCodesFollowTheClock() async throws {
        try store.update { _ in ("rfc 8 \(rfcKey)\nsix 6 \(rfcKey)\n", ()) }
        let model = model()
        XCTAssertEqual(model.keys.map(\.name), ["rfc", "six"])
        XCTAssertEqual(code(model, "rfc"), "07081804")
        XCTAssertEqual(code(model, "six"), "081804")
        XCTAssertEqual(model.secondsRemaining, 1)

        // Half a second later: the same step, and the next tick is due when the second ends.
        time += 0.5
        XCTAssertEqual(model.tick(), 0.5, accuracy: 0.001)
        XCTAssertEqual(code(model, "rfc"), "07081804")
        XCTAssertEqual(model.secondsRemaining, 1)

        // The step is over: the codes are the next ones (RFC 6238 gives 14050471 for 1111111111).
        time = rfcTime + 1
        XCTAssertEqual(model.tick(), 1, accuracy: 0.001)
        XCTAssertEqual(code(model, "rfc"), "14050471")
        XCTAssertEqual(model.secondsRemaining, 30)
        time += 12
        model.tick()
        XCTAssertEqual(code(model, "rfc"), "14050471")
        XCTAssertEqual(model.secondsRemaining, 18)
    }

    @MainActor func testWithinAStepTheKeychainIsNotReadAgain() async throws {
        try store.update { _ in ("rfc 8 \(rfcKey)\n", ()) }
        let model = model()
        secrets.failsReads = true
        time += 0.9
        model.tick()
        XCTAssertNil(model.errorMessage)
        XCTAssertEqual(code(model, "rfc"), "07081804")
    }

    @MainActor func testAddRenameRemove() async throws {
        let model = model()
        XCTAssertEqual(model.keys, [])

        XCTAssertTrue(model.add(draft(" github ", "jbsw y3dp ehpk 3pxp")))
        XCTAssertTrue(model.add(draft("aws/prod", rfcKey, digits: 8)))
        XCTAssertTrue(model.add(draft("bank", hello, digits: 7, counter: 0)))
        XCTAssertEqual(
            try store.text(),
            "github 6 \(hello)\naws/prod 8 \(rfcKey)\nbank 7 \(hello) 00000000000000000000\n")
        XCTAssertEqual(model.keys.map(\.name), ["aws/prod", "bank", "github"])
        XCTAssertEqual(
            model.keys[0], KeyItem(name: "aws/prod", digits: 8, isCounterBased: false, code: "07081804"))
        XCTAssertEqual(model.keys[1], KeyItem(name: "bank", digits: 7, isCounterBased: true, code: nil))

        // Refused, with the core's own sentence, and nothing stored.
        let before = try store.text()
        for (bad, message) in [
            (draft("github", hello), "there is already a key named github"),
            (draft("my key", hello), "a name is made of letters, digits, - / and = (no space, no accent)"),
            (draft("", hello), "the name is empty"),
            (draft("x", "not base32!"), "the key is not valid base32 (letters A to Z and digits 2 to 7)"),
            (draft("x", hello, digits: 9), "codes are 6, 7 or 8 digits"),
            (draft("x", hello, digits: 300), "codes are 6, 7 or 8 digits"),
        ] {
            model.errorMessage = nil
            XCTAssertFalse(model.add(bad))
            XCTAssertEqual(model.errorMessage, message)
        }
        XCTAssertFalse(model.rename("github", to: "bank"))
        XCTAssertEqual(model.errorMessage, "there is already a key named bank")
        XCTAssertFalse(model.remove("nope"))
        XCTAssertEqual(model.errorMessage, "no such key nope")
        XCTAssertEqual(try store.text(), before)

        XCTAssertTrue(model.rename("github", to: " GitHub-me "))
        XCTAssertTrue(model.remove("aws/prod"))
        XCTAssertEqual(try store.text(), "GitHub-me 6 \(hello)\nbank 7 \(hello) 00000000000000000000\n")
        XCTAssertEqual(model.keys.map(\.name), ["GitHub-me", "bank"])
    }

    @MainActor func testCounterBasedCodes() async throws {
        try store.update { _ in ("hotp 6 \(rfcKey) 00000000000000000000\ntotp 6 \(hello)\n", ()) }
        let model = model()
        XCTAssertNil(code(model, "hotp"))

        // RFC 4226: counters 1 and 2. The counter is in the store before the code is on screen.
        model.nextCode(for: "hotp")
        XCTAssertEqual(code(model, "hotp"), "287082")
        XCTAssertEqual(try store.text(), "hotp 6 \(rfcKey) 00000000000000000001\ntotp 6 \(hello)\n")
        model.nextCode(for: "hotp")
        XCTAssertEqual(code(model, "hotp"), "359152")

        // The code stays as time passes, follows its key when renamed, and goes with it.
        time += 60
        model.tick()
        XCTAssertEqual(code(model, "hotp"), "359152")
        XCTAssertTrue(model.rename("hotp", to: "bank"))
        XCTAssertEqual(code(model, "bank"), "359152")
        XCTAssertTrue(model.remove("bank"))
        XCTAssertTrue(model.add(draft("bank", rfcKey, counter: 7)))
        XCTAssertNil(code(model, "bank"))

        model.nextCode(for: "totp")
        XCTAssertEqual(model.errorMessage, "totp is a time-based key: it has no counter")
    }

    @MainActor func testACounterThatCannotBeStoredGivesNoCode() async throws {
        try store.update { _ in ("hotp 6 \(rfcKey) 00000000000000000000\n", ()) }
        let model = model()
        model.nextCode(for: "hotp")
        XCTAssertEqual(code(model, "hotp"), "287082")

        // A code shown but not counted would be shown again next time, and refused by the server.
        secrets.failsWrites = true
        model.nextCode(for: "hotp")
        XCTAssertNotNil(model.errorMessage)
        XCTAssertEqual(code(model, "hotp"), "287082", "still the last one that was counted")
        XCTAssertEqual(try store.text(), "hotp 6 \(rfcKey) 00000000000000000001\n")

        secrets.failsWrites = false
        model.nextCode(for: "hotp")
        XCTAssertEqual(code(model, "hotp"), "359152")
    }

    @MainActor func testAnUnreadableKeychainIsNotWrittenOver() async throws {
        try store.update { _ in ("a 6 \(hello)\n", ()) }
        secrets.failsReads = true
        let model = model()
        XCTAssertEqual(model.keys, [])
        XCTAssertNotNil(model.errorMessage)

        XCTAssertFalse(model.add(draft("b", hello)))
        XCTAssertNil(model.importText("c 6 \(hello)\n"))
        XCTAssertNil(model.exportText())
        secrets.failsReads = false
        XCTAssertEqual(try store.text(), "a 6 \(hello)\n")
    }

    @MainActor func testCopy() async throws {
        try store.update { _ in ("hotp 6 \(rfcKey) 00000000000000000000\nrfc 8 \(rfcKey)\n", ()) }
        let model = model()

        model.copy("hotp") // no code yet: nothing to copy
        model.copy("nope")
        XCTAssertTrue(clipboard.copies.isEmpty)
        XCTAssertNil(model.copied)

        model.copy("rfc")
        XCTAssertEqual(clipboard.copies.map(\.text), ["07081804"], "the digits alone, not grouped for the eye")
        XCTAssertEqual(clipboard.copies[0].expiresAfter, 60)
        XCTAssertEqual(model.copied, "rfc")

        // The row says so for two seconds.
        time += 1.9
        model.tick()
        XCTAssertEqual(model.copied, "rfc")
        time += 0.2
        model.tick()
        XCTAssertNil(model.copied)

        model.nextCode(for: "hotp")
        model.copy("hotp")
        XCTAssertEqual(clipboard.copies.last?.text, "287082")
    }

    @MainActor func testImportAndExport() async throws {
        let model = model()
        XCTAssertEqual(model.exportText(), "")

        // Into nothing: byte for byte, with what is not a key, and exported the same.
        let file = "b 6 jbswy3dpehpk3pxp\n# a note\nhotp 6 \(rfcKey) 00000000000000000003\n"
        let first = try XCTUnwrap(model.importFile(Data(file.utf8)))
        XCTAssertEqual(first.added, ["b", "hotp"])
        XCTAssertEqual(first.summary, "2 keys added. 1 line was not a key.")
        XCTAssertEqual(model.exportText(), file)
        XCTAssertEqual(model.keys.map(\.name), ["b", "hotp"])
        XCTAssertEqual(model.skippedLines, [SkippedLine(line: 2, reason: "not a key: expected `name digits key`, separated by one space")])

        // Then merged: new keys are added, a counter only goes forward, and a name in use is not written over.
        let second = try XCTUnwrap(
            model.importText("b 6 \(rfcKey)\nhotp 6 \(rfcKey) 00000000000000000009\nc 8 \(hello)\n"))
        XCTAssertEqual(
            second.summary,
            "1 key added. 1 counter moved forward. Kept as they were, because the file has another key under the same name: b.")
        XCTAssertEqual(
            model.exportText(),
            "b 6 jbswy3dpehpk3pxp\n# a note\nhotp 6 \(rfcKey) 00000000000000000009\nc 8 \(hello)\n")

        // Nothing to take: nothing written.
        let writes = secrets.writes
        let third = try XCTUnwrap(model.importText("c 8 \(hello)\nhotp 6 \(rfcKey) 00000000000000000001\n\n"))
        XCTAssertEqual(third.summary, "0 keys added. 2 were already there.")
        XCTAssertEqual(secrets.writes, writes)

        XCTAssertNil(model.importFile(Data([0xff, 0xfe, 0x00])))
        XCTAssertEqual(model.errorMessage, "This is not a 2fa file: it is not text.")
    }

    @MainActor func testDraftFromAQRCode() async throws {
        let model = model()
        let scanned = try XCTUnwrap(model.draft(
            fromURI: "otpauth://totp/ACME%20Co:john.doe@email.com?secret=gezdgnbvgy3tqojqgezdgnbvgy3tqojq&issuer=ACME%20Co&digits=8"))
        XCTAssertEqual(scanned.name, "ACME-Co-john-doe-email-com")
        XCTAssertEqual(scanned.secret, rfcKey)
        XCTAssertEqual(scanned.digits, 8)
        XCTAssertFalse(scanned.isCounterBased)
        XCTAssertTrue(model.add(scanned))
        XCTAssertEqual(code(model, "ACME-Co-john-doe-email-com"), "07081804")

        let counted = try XCTUnwrap(model.draft(fromURI: "otpauth://hotp/bank?secret=\(rfcKey)&counter=1"))
        XCTAssertTrue(counted.isCounterBased)
        XCTAssertEqual(counted.counter, 1)
        XCTAssertTrue(model.add(counted))
        model.nextCode(for: "bank")
        XCTAssertEqual(code(model, "bank"), "359152") // counter 2

        // What would give wrong codes is refused, and says why.
        for (uri, message) in [
            ("otpauth://totp/a?secret=\(hello)&algorithm=SHA256", "not supported: the SHA256 algorithm (only SHA1)"),
            ("otpauth://totp/a?secret=\(hello)&period=60", "not supported: a period of 60 seconds (only 30)"),
            ("https://example.com", "not a usable otpauth:// URI: no otpauth:// at the start"),
        ] {
            XCTAssertNil(model.draft(fromURI: uri))
            XCTAssertEqual(model.errorMessage, message)
        }
    }

    @MainActor func testLock() async throws {
        try store.update { _ in ("hotp 6 \(rfcKey) 00000000000000000000\nrfc 8 \(rfcKey)\n", ()) }

        // Off by default: open, and `lock` does nothing.
        var model = model()
        XCTAssertFalse(model.lockOnOpen)
        XCTAssertFalse(model.isLocked)
        model.lock()
        XCTAssertFalse(model.isLocked)

        // It is only turned on by a device that can then unlock.
        authenticator.answer = false
        await model.setLockOnOpen(true)
        XCTAssertFalse(model.lockOnOpen)
        XCTAssertNotNil(model.errorMessage)
        authenticator.answer = true
        await model.setLockOnOpen(true)
        XCTAssertTrue(model.lockOnOpen)
        XCTAssertFalse(model.isLocked, "not locked under the hand that just turned it on")

        // Leaving the app forgets everything shown.
        model.nextCode(for: "hotp")
        model.copy("rfc")
        model.lock()
        XCTAssertTrue(model.isLocked)
        XCTAssertEqual(model.keys, [])
        XCTAssertNil(model.copied)

        // Locked: nothing comes out, nothing goes in.
        model.tick()
        XCTAssertEqual(model.keys, [])
        XCTAssertNil(model.exportText())
        XCTAssertNil(model.importText("x 6 \(hello)\n"))
        XCTAssertFalse(model.add(draft("x", hello)))
        XCTAssertFalse(model.remove("rfc"))
        model.nextCode(for: "hotp")
        model.copy("rfc")
        XCTAssertEqual(clipboard.copies.count, 1)
        XCTAssertEqual(try store.text(), "hotp 6 \(rfcKey) 00000000000000000001\nrfc 8 \(rfcKey)\n")

        authenticator.answer = false
        await model.unlock()
        XCTAssertTrue(model.isLocked)
        authenticator.answer = true
        await model.unlock()
        XCTAssertFalse(model.isLocked)
        XCTAssertEqual(code(model, "rfc"), "07081804")
        XCTAssertNil(code(model, "hotp"), "a code from before the lock is not shown again")

        // The setting is kept: the next launch starts locked.
        model = self.model()
        XCTAssertTrue(model.isLocked)
        XCTAssertEqual(model.keys, [])
        await model.unlock()
        XCTAssertEqual(model.keys.count, 2)

        let asked = authenticator.reasons.count
        await model.setLockOnOpen(false)
        XCTAssertEqual(authenticator.reasons.count, asked, "turning it off asks nothing")
        XCTAssertFalse(self.model().isLocked)
    }

    @MainActor func testCodesAreGroupedForTheEye() async throws {
        XCTAssertEqual(groupedCode("123456"), "123 456")
        XCTAssertEqual(groupedCode("1234567"), "1234 567")
        XCTAssertEqual(groupedCode("12345678"), "1234 5678")
        XCTAssertEqual(groupedCode(""), "")
    }
}
