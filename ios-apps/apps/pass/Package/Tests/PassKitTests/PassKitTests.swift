import Foundation
import XCTest
@testable import PassKit

final class SettingsTests: XCTestCase {
    func testSSHNeedsAPinnedHostKeyAndADeviceKey() {
        var settings = PassSettings()
        XCTAssertFalse(settings.isRemoteComplete)
        settings.url = " git@relay.example:store.git\n"
        XCTAssertFalse(settings.isRemoteComplete, "no SSH before the host key is pinned")
        settings.sshHostKey = "SHA256:abc"
        XCTAssertTrue(settings.isRemoteComplete)
        XCTAssertNil(settings.remote(sshPrivateKey: nil, httpsToken: "t"), "no device key yet")
        XCTAssertEqual(
            settings.remote(sshPrivateKey: "KEY", httpsToken: "t"),
            .ssh(url: "git@relay.example:store.git", privateKey: "KEY", hostKey: "SHA256:abc"))
        settings.deviceName = ""
        XCTAssertFalse(settings.isRemoteComplete, "the device's branch needs a name")
    }

    func testHTTPSTrustFollowsTheURL() {
        var settings = PassSettings()
        settings.transport = .https
        settings.url = "https://git.example/alice/store.git"
        settings.httpsUsername = "alice"
        XCTAssertFalse(settings.isRemoteComplete, "https is never trusted by omission")
        XCTAssertNil(settings.trust)

        let pin = String(repeating: "ab", count: 32)
        settings.pinSHA256 = pin
        XCTAssertEqual(settings.trust, .pinnedOnly(sha256: pin))
        settings.verifyChain = true
        XCTAssertEqual(settings.trust, .pinnedWithCa(sha256: pin, extraCaPem: nil))
        settings.trustPublicCAs = true
        XCTAssertEqual(settings.trust, .publicCa)
        XCTAssertEqual(
            settings.remote(sshPrivateKey: "KEY", httpsToken: "t0ken"),
            .http(url: "https://git.example/alice/store.git", username: "alice", password: "t0ken", trust: .publicCa))

        // Plain http: no trust to speak of, and the token stays home.
        settings.url = "http://10.0.0.2:8080/store.git"
        XCTAssertTrue(settings.isPlainHTTP)
        XCTAssertNil(settings.trust)
        XCTAssertEqual(
            settings.remote(sshPrivateKey: nil, httpsToken: "t0ken"),
            .http(url: "http://10.0.0.2:8080/store.git", username: "alice", password: nil, trust: nil))
    }

    func testStoreConfig() {
        var settings = PassSettings()
        settings.deviceName = "phone"
        settings.otherPublicKeys = ["BOB"]
        settings.signingKeys = ["ABCD"]
        settings.requireSignedUpdates = true
        let config = settings.storeConfig(directory: URL(fileURLWithPath: "/store"), ownPublicKey: "ALICE")
        XCTAssertEqual(config.directory, "/store")
        XCTAssertEqual(config.publicKeys, ["ALICE", "BOB"])
        XCTAssertEqual(config.signingKeys, ["ABCD"])
        XCTAssertTrue(config.requireSignedUpdates)
        // Commits need an author: there is one even when none was typed.
        XCTAssertEqual(config.authorName, "pass on phone")
        XCTAssertEqual(config.authorEmail, "phone@pass.invalid")
        settings.authorName = "Alice"
        settings.authorEmail = "alice@example.org"
        XCTAssertEqual(settings.storeConfig(directory: URL(fileURLWithPath: "/store"), ownPublicKey: nil).authorName, "Alice")
        XCTAssertNoThrow(try PassService(config: config.with(publicKeys: [])))
        XCTAssertThrowsError(try PassService(config: config)) {
            XCTAssertTrue(message(for: $0).contains("not an OpenPGP public key"), message(for: $0))
        }
    }

    func testStoreRoundTripAndErase() throws {
        let suite = "pass-tests-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = SettingsStore(defaults: defaults, secrets: InMemorySecretStore())
        XCTAssertEqual(store.load(), PassSettings())

        var settings = PassSettings()
        settings.url = "git@relay.example:store.git"
        settings.sshHostKey = "SHA256:abc"
        settings.passphrasePolicy = .keychainWithBiometrics
        settings.lockAfter = 30
        try store.save(settings)
        XCTAssertEqual(store.load(), settings)

        try store.setSecret("KEY", for: .sshPrivateKey)
        XCTAssertEqual(store.secret(.sshPrivateKey), "KEY")
        try store.setSecret("", for: .sshPrivateKey)
        XCTAssertNil(store.secret(.sshPrivateKey), "an empty secret is no secret")

        try store.setSecret("t", for: .httpsToken)
        try store.erase()
        XCTAssertEqual(store.load(), PassSettings())
        XCTAssertNil(store.secret(.httpsToken))
    }

    func testOnlyThePassphraseIsBehindBiometry() {
        XCTAssertEqual(SecretAccount.allCases.filter(\.requiresBiometry), [.passphrase])
    }
}

extension StoreConfig {
    func with(publicKeys: [String]) -> StoreConfig {
        var copy = self
        copy.publicKeys = publicKeys
        return copy
    }
}

final class PresentationTests: XCTestCase {
    func testTree() {
        let tree = storeTree(["bank", "Email/work", "Email/Perso", "email", "web/shops/corner", "web/a", "Email"])
        XCTAssertEqual(tree.map(\.id), ["Email/", "web/", "bank", "Email", "email"])
        let email = tree[0]
        XCTAssertTrue(email.isFolder)
        XCTAssertEqual(email.children.map(\.path), ["Email/Perso", "Email/work"])
        XCTAssertEqual(email.entryCount, 2)
        let web = tree[1]
        XCTAssertEqual(web.children.map(\.id), ["web/shops/", "web/a"])
        XCTAssertEqual(web.children[0].children.map(\.path), ["web/shops/corner"])
        XCTAssertEqual(web.entryCount, 2)
        // An entry and a folder of the same name are two rows.
        XCTAssertFalse(tree[3].isFolder)
        XCTAssertEqual(tree[3].path, "Email")
        XCTAssertEqual(storeTree([]), [])
    }

    func testSyncStateInWords() {
        func state(unmerged: UInt32 = 0, unpushed: UInt32 = 0, behind: UInt32 = 0, repository: Bool = true) -> SyncState {
            SyncState(
                isRepository: repository, branch: "main", deviceBranch: "devices/phone", lastSubject: "", lastTime: 0,
                unmerged: unmerged, unpushed: unpushed, behind: behind)
        }
        XCTAssertEqual(describe(state()), "Up to date with main.")
        XCTAssertEqual(describe(state(unmerged: 2, unpushed: 2)), "2 change(s) to send.")
        XCTAssertEqual(describe(state(unmerged: 3, unpushed: 1)), "1 change(s) to send, 2 change(s) sent, waiting to be merged.")
        XCTAssertEqual(
            describe(state(unmerged: 1, behind: 4)), "1 change(s) sent, waiting to be merged, 4 new change(s) on main.")
        XCTAssertEqual(describe(state(repository: false)), "Not synced: the store is not a git repository.")
    }

    func testFingerprintGrouping() {
        XCTAssertEqual(groupedFingerprint("ABCD1234EF"), "ABCD 1234 EF")
        XCTAssertEqual(groupedFingerprint(""), "")
    }

    func testErrorsFromTheCoreAreReadable() {
        XCTAssertEqual(message(for: PassError.NotFound(name: "bank")), "bank is not in the password store")
        XCTAssertEqual(message(for: PassError.WrongPassphrase), "wrong passphrase")
        XCTAssertThrowsError(try inspectKey(key: "nope")) {
            XCTAssertTrue(message(for: $0).hasPrefix("not an OpenPGP public key"), message(for: $0))
        }
    }

    func testOneTimeCodes() throws {
        // RFC 6238, appendix B.
        let uri = "otpauth://totp/x?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&digits=8"
        XCTAssertEqual(try otpCode(uri: uri, unixTime: 59), OtpCode(code: "94287082", period: 30, remaining: 1))
    }
}

/// Remembers what was copied, and until when.
final class FakePasteboard: SecretPasteboard, @unchecked Sendable {
    private let lock = NSLock()
    private var copies: [(text: String, expiresAt: Date)] = []

    func copy(_ text: String, expiresAt: Date) {
        lock.lock()
        copies.append((text, expiresAt))
        lock.unlock()
    }

    var last: (text: String, expiresAt: Date)? {
        lock.lock()
        defer { lock.unlock() }
        return copies.last
    }
}

/// A clock the tests move by hand.
final class FakeClock: @unchecked Sendable {
    private let lock = NSLock()
    private var current = Date(timeIntervalSince1970: 1_800_000_000)

    var now: Date {
        lock.lock()
        defer { lock.unlock() }
        return current
    }

    func advance(_ seconds: TimeInterval) {
        lock.lock()
        current += seconds
        lock.unlock()
    }
}

@MainActor
final class Fixture {
    let directory: URL
    let pasteboard = FakePasteboard()
    let clock = FakeClock()
    let store: SettingsStore
    private let suite = "pass-tests-\(UUID().uuidString)"

    init() throws {
        directory = FileManager.default.temporaryDirectory.appendingPathComponent("pass-kit-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        store = SettingsStore(defaults: UserDefaults(suiteName: suite)!, secrets: InMemorySecretStore())
    }

    /// A model over the same settings, secrets and store directory: what a relaunch of the app gets.
    func model() -> AppModel {
        let clock = clock
        return AppModel(
            store: store, storeDirectory: directory.appendingPathComponent("store"), pasteboard: pasteboard,
            now: { clock.now })
    }

    func cleanUp() {
        try? FileManager.default.removeItem(at: directory)
        UserDefaults().removePersistentDomain(forName: suite)
    }
}

@MainActor
final class ModelTests: XCTestCase {
    func testAFreshInstallAsksForAKey() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanUp() }
        let model = fixture.model()
        XCTAssertEqual(model.stage, .needsKey)
        XCTAssertFalse(model.isUnlocked)
        XCTAssertFalse(model.canUnlockWithBiometrics)
        XCTAssertEqual(model.entries, [])
        XCTAssertNil(model.searchResults)

        let unlocked = await model.unlock(passphrase: "x")
        XCTAssertFalse(unlocked)
        XCTAssertEqual(model.errorMessage, "No secret key on this device.")
        model.errorMessage = nil
        let imported = await model.importSecretKey("not a key", passphrase: "x", policy: .askEachTime)
        XCTAssertFalse(imported)
        XCTAssertNotNil(model.errorMessage)
        XCTAssertNil(fixture.store.secret(.openPGPSecretKey), "nothing is kept of a key that was refused")

        await model.sync()
        XCTAssertEqual(model.errorMessage, "The relay is not set up yet.")
    }

    // (async like its neighbours: on Linux, XCTest cannot list tests of both kinds in a main-actor class.)
    func testTheDeviceKeyIsMadeOnceAndItsPrivateHalfStaysInTheKeychain() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanUp() }
        let model = fixture.model()
        XCTAssertNil(model.deviceKey)
        XCTAssertNil(model.authorizedKeysLine)
        model.makeDeviceKey()
        let key = try XCTUnwrap(model.deviceKey)
        XCTAssertTrue(key.publicKey.hasPrefix("ssh-ed25519 AAAA"))
        XCTAssertTrue(key.publicKey.hasSuffix(" pass on phone"))
        XCTAssertEqual(key.privateKey, "", "the model does not hand the private half around")
        XCTAssertTrue(try XCTUnwrap(fixture.store.secret(.sshPrivateKey)).contains("OPENSSH PRIVATE KEY"))
        XCTAssertEqual(
            model.authorizedKeysLine,
            #"command="PASS_DEVICE=phone exec git-shell -c \"$SSH_ORIGINAL_COMMAND\"",restrict \#(key.publicKey)"#)

        model.makeDeviceKey()
        XCTAssertEqual(model.deviceKey, key, "asking again does not replace the key the relay knows")
        XCTAssertEqual(fixture.model().deviceKey, key, "a relaunch finds it")
    }
}

/// Swift → UniFFI → Rust → a real relay (`git http-backend`), on a store made by
/// gpg and pass.py. `swift-test.sh` makes the fixtures, sets these, and checks
/// afterwards with gpg and git what was pushed here.
@MainActor
final class EndToEndTests: XCTestCase {
    private var fixture: Fixture!
    private var secretKey = ""
    private var passphrase = ""
    private var fingerprint = ""
    private var url = ""
    private var olderURL = ""

    override func setUp() async throws {
        let env = ProcessInfo.processInfo.environment
        guard let fixtures = env["PASS_E2E_FIXTURES"], let url = env["PASS_E2E_URL"],
              let olderURL = env["PASS_E2E_OLDER_URL"], let passphrase = env["PASS_E2E_PASSPHRASE"] else {
            throw XCTSkip("no fixtures: run `make swift-test`")
        }
        let directory = URL(fileURLWithPath: fixtures)
        secretKey = try String(contentsOf: directory.appendingPathComponent("secret.asc"), encoding: .utf8)
        fingerprint = try String(contentsOf: directory.appendingPathComponent("fingerprint"), encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        (self.url, self.olderURL, self.passphrase) = (url, olderURL, passphrase)
        fixture = try Fixture()
    }

    override func tearDown() async throws {
        fixture?.cleanUp()
    }

    private func settings(url: String, device: String) -> PassSettings {
        var settings = PassSettings()
        settings.transport = .https
        settings.url = url
        settings.deviceName = device
        settings.lockAfter = 120
        settings.clipboardExpiry = 45
        return settings
    }

    /// Imports the key, points at the relay and clones: a model that is set up and unlocked.
    private func setUpModel(url: String, device: String) async throws -> AppModel {
        let model = fixture.model()
        let imported = await model.importSecretKey(secretKey, passphrase: passphrase, policy: .askEachTime)
        XCTAssertTrue(imported, model.errorMessage ?? "")
        XCTAssertEqual(model.stage, .needsRemote)
        model.save(settings(url: url, device: device))
        XCTAssertEqual(model.stage, .needsClone)
        let cloned = await model.cloneStore()
        XCTAssertTrue(cloned, model.errorMessage ?? "")
        XCTAssertEqual(model.stage, .ready)
        return model
    }

    func testImportingTheKey() async throws {
        let model = fixture.model()
        var imported = await model.importSecretKey(secretKey, passphrase: "wrong", policy: .askEachTime)
        XCTAssertFalse(imported)
        XCTAssertEqual(model.errorMessage, "wrong passphrase")
        XCTAssertEqual(model.stage, .needsKey)
        XCTAssertNil(fixture.store.secret(.openPGPSecretKey))

        let publicKey = try publicKeyOf(secretKey: secretKey)
        model.errorMessage = nil
        imported = await model.importSecretKey(publicKey, passphrase: passphrase, policy: .askEachTime)
        XCTAssertFalse(imported)
        XCTAssertTrue(try XCTUnwrap(model.errorMessage).hasPrefix("This is a public key."))

        imported = await model.importSecretKey(secretKey, passphrase: passphrase, policy: .askEachTime)
        XCTAssertTrue(imported)
        XCTAssertEqual(model.keyInfo?.fingerprint, fingerprint)
        XCTAssertEqual(model.keyInfo?.needsPassphrase, true)
        XCTAssertTrue(model.isUnlocked, "importing unlocks: the passphrase was just typed")
        XCTAssertNil(fixture.store.secret(.passphrase), "asked each time: it is nowhere on the phone")
        XCTAssertFalse(model.canUnlockWithBiometrics)

        // A relaunch has the key, and is locked.
        let relaunched = fixture.model()
        XCTAssertEqual(relaunched.keyInfo?.fingerprint, fingerprint)
        XCTAssertFalse(relaunched.isUnlocked)
        let glance = await relaunched.unlockWithBiometrics()
        XCTAssertFalse(glance)
        XCTAssertNil(relaunched.errorMessage)
    }

    func testFaceIDUnlocksWithTheKeptPassphrase() async throws {
        let model = fixture.model()
        let imported = await model.importSecretKey(secretKey, passphrase: passphrase, policy: .keychainWithBiometrics)
        XCTAssertTrue(imported)
        XCTAssertTrue(model.canUnlockWithBiometrics)
        model.lock()
        XCTAssertFalse(model.isUnlocked)
        let unlocked = await model.unlockWithBiometrics()
        XCTAssertTrue(unlocked)
        XCTAssertTrue(model.isUnlocked)

        // Going back to typing it removes it from the keychain.
        var settings = model.settings
        settings.passphrasePolicy = .askEachTime
        model.save(settings)
        XCTAssertNil(fixture.store.secret(.passphrase))
        model.lock()
        let again = await model.unlockWithBiometrics()
        XCTAssertFalse(again)
    }

    func testBrowseReadLockAndCopy() async throws {
        let model = try await setUpModel(url: url, device: "reader")
        XCTAssertEqual(model.entries, ["Email/work", "bank", "wifi"])
        XCTAssertEqual(model.tree.map(\.id), ["Email/", "bank", "wifi"])
        XCTAssertEqual(model.recipientFiles.map(\.path), [".gpg-id"])
        XCTAssertEqual(model.recipientFiles.first?.unknown, [], "this device's own key is the recipient")
        model.searchText = " WORK  em "
        XCTAssertEqual(model.searchResults, ["Email/work"])
        model.searchText = ""
        XCTAssertNil(model.searchResults)

        let shown = await model.show("bank")
        let bank = try XCTUnwrap(shown)
        XCTAssertEqual(bank.password, "hunter2")
        XCTAssertEqual(bank.fields, [EntryField(name: "login", value: "alice")])
        XCTAssertEqual(bank.otpUris.count, 1)
        XCTAssertEqual(model.oneTimeCode(for: bank.otpUris[0])?.code.count, 6)

        model.copy(bank.password)
        let copied = try XCTUnwrap(fixture.pasteboard.last)
        XCTAssertEqual(copied.text, "hunter2")
        XCTAssertEqual(copied.expiresAt, fixture.clock.now.addingTimeInterval(45))

        // Use keeps it open; two minutes without use lock it.
        fixture.clock.advance(100)
        model.lockIfExpired()
        XCTAssertTrue(model.isUnlocked)
        model.userDidInteract()
        fixture.clock.advance(100)
        model.lockIfExpired()
        XCTAssertTrue(model.isUnlocked)
        fixture.clock.advance(121)
        let late = await model.show("bank")
        XCTAssertNil(late, "the timeout is checked before decrypting, not only by the timer")
        XCTAssertFalse(model.isUnlocked)
        XCTAssertEqual(model.errorMessage, "The store is locked.")
        XCTAssertEqual(model.entries.count, 3, "the names stay: listing needs no key")

        model.errorMessage = nil
        var unlocked = await model.unlock(passphrase: "wrong")
        XCTAssertFalse(unlocked)
        XCTAssertEqual(model.errorMessage, "wrong passphrase")
        unlocked = await model.unlock(passphrase: passphrase)
        XCTAssertTrue(unlocked)
        model.didEnterBackground()
        XCTAssertFalse(model.isUnlocked, "leaving the app locks at once")

        let missing = await fixture.model().show("bank")
        XCTAssertNil(missing, "a relaunch is locked")
    }

    /// What `swift-test.sh` then looks for on the relay, with git and gpg.
    func testChangesGoToTheDeviceBranch() async throws {
        let model = try await setUpModel(url: url, device: "swift")
        let added = await model.add("swift/added", content: "from-swift\nlogin: swift")
        XCTAssertTrue(added, model.errorMessage ?? "")
        let duplicate = await model.add("swift/added", content: "again")
        XCTAssertFalse(duplicate)
        XCTAssertEqual(model.errorMessage, "an entry already exists for swift/added")
        model.errorMessage = nil

        let generated = await model.generate("swift/generated", length: 24, symbols: false)
        XCTAssertEqual(generated?.count, 24)
        let saved = await model.save("bank", content: "hunter3\nlogin: alice\n")
        XCTAssertTrue(saved)
        let renamed = await model.rename("wifi", to: "home/wifi")
        XCTAssertTrue(renamed)
        let deleted = await model.delete("Email/work")
        XCTAssertTrue(deleted)
        XCTAssertEqual(model.entries, ["bank", "home/wifi", "swift/added", "swift/generated"])
        let reread = await model.show("swift/generated")
        XCTAssertEqual(reread?.password, generated)
        let edited = await model.content(of: "bank")
        XCTAssertEqual(edited, "hunter3\nlogin: alice\n")
        XCTAssertEqual(model.syncState.map(describe), "5 change(s) to send.")

        // Syncing needs no key: it goes on while locked.
        model.lock()
        await model.sync()
        XCTAssertNil(model.errorMessage)
        XCTAssertEqual(model.syncMessage, "Nothing new. Sent 5 change(s) to devices/swift.")
        XCTAssertEqual(model.syncState.map(describe), "5 change(s) sent, waiting to be merged.")
        XCTAssertFalse(model.isSyncing)
        await model.sync()
        XCTAssertEqual(model.syncMessage, "Nothing new.")
    }

    func testTakingTheMainBranchAndGivingChangesUp() async throws {
        // Cloned from the relay as it was two changes ago, then pointed at the relay as it is.
        let model = try await setUpModel(url: olderURL, device: "late")
        XCTAssertEqual(model.entries, ["bank"])
        model.save(settings(url: url, device: "late"))
        XCTAssertEqual(model.stage, .ready)
        await model.sync()
        XCTAssertNil(model.errorMessage)
        XCTAssertEqual(model.syncMessage, "Took 2 change(s).")
        XCTAssertEqual(model.entries, ["Email/work", "bank", "wifi"])
        XCTAssertNil(model.divergence)

        // The same from a device that has changes of its own: nothing is merged here.
        let other = try Fixture()
        defer { other.cleanUp() }
        let behind = other.model()
        _ = await behind.importSecretKey(secretKey, passphrase: passphrase, policy: .askEachTime)
        behind.save(settings(url: olderURL, device: "diverged"))
        _ = await behind.cloneStore()
        _ = await behind.add("mine", content: "x")
        behind.save(settings(url: url, device: "diverged"))
        await behind.sync()
        XCTAssertNil(behind.errorMessage)
        XCTAssertEqual(behind.divergence, Divergence(unmerged: 1, behind: 2))
        XCTAssertEqual(behind.entries, ["bank", "mine"], "nothing was taken")
        XCTAssertEqual(
            behind.syncMessage,
            "2 new change(s) cannot be taken until this device's 1 are merged on the computer. Sent 1 change(s) to devices/diverged.")

        await behind.discardLocalChanges()
        XCTAssertNil(behind.errorMessage)
        XCTAssertNil(behind.divergence)
        XCTAssertEqual(behind.syncMessage, "Dropped 1 change(s) of this device.")
        XCTAssertEqual(behind.entries, ["Email/work", "bank", "wifi"])
    }

    func testErasingLeavesNothing() async throws {
        let model = try await setUpModel(url: url, device: "leaving")
        model.makeDeviceKey()
        model.setHTTPSToken("t0ken")
        XCTAssertTrue(model.hasHTTPSToken)
        model.eraseEverything()
        XCTAssertEqual(model.stage, .needsKey)
        XCTAssertFalse(model.isUnlocked)
        XCTAssertEqual(model.entries, [])
        XCTAssertNil(model.deviceKey)
        XCTAssertFalse(model.hasHTTPSToken)
        for account in SecretAccount.allCases {
            XCTAssertNil(fixture.store.secret(account), account.rawValue)
        }
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.directory.appendingPathComponent("store").path))
        XCTAssertEqual(fixture.model().stage, .needsKey)
    }
}
