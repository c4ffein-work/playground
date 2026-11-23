import Foundation
import XCTest
@testable import PassKit

final class SettingsTests: XCTestCase {
    func testTheSyncNeedsADropZoneItsPinnedKeyAndAnOwner() {
        var settings = PassSettings()
        XCTAssertFalse(settings.isSyncComplete)
        settings.dropZoneURL = " sftp://drop@zone.example:2222\n"
        settings.dropZoneHostKey = "SHA256:abc"
        XCTAssertFalse(settings.isSyncComplete, "nobody to sync with")
        settings.peers = [PeerSettings(name: "tablet", fingerprint: "AB", publicKey: "KEY", isOwner: false)]
        XCTAssertFalse(settings.isSyncComplete, "a member is not the owner")
        XCTAssertNil(settings.owner)
        settings.peers.append(PeerSettings(name: "pc", fingerprint: "CD", publicKey: "KEY", isOwner: true))
        XCTAssertTrue(settings.isSyncComplete)
        XCTAssertEqual(settings.owner?.name, "pc")

        XCTAssertNil(settings.dropZone(sshPrivateKey: nil), "no SSH key yet")
        let zone = settings.dropZone(sshPrivateKey: "KEY")
        XCTAssertEqual(zone?.url, "sftp://drop@zone.example:2222")
        XCTAssertEqual(zone?.hostKey, "SHA256:abc")

        settings.dropZoneHostKey = nil
        XCTAssertFalse(settings.isSyncComplete, "nothing is sent to a server whose key is not pinned")
        settings.dropZoneHostKey = "SHA256:abc"
        settings.deviceName = "My Phone"
        XCTAssertFalse(settings.isSyncComplete, "not a device name")
    }

    func testDeviceNames() {
        for good in ["phone", "pc", "0", "alices-iphone-15", String(repeating: "a", count: 32)] {
            XCTAssertTrue(PassSettings.isDeviceName(good), good)
        }
        for bad in ["", "-phone", "Phone", "my phone", "a/b", "é", String(repeating: "a", count: 33)] {
            XCTAssertFalse(PassSettings.isDeviceName(bad), bad)
        }
    }

    func testStoreConfig() {
        var settings = PassSettings()
        settings.peers = [PeerSettings(name: "pc", fingerprint: "ABCD", publicKey: "PC-KEY", isOwner: true)]
        let config = settings.storeConfig(directory: URL(fileURLWithPath: "/store"), ownPublicKey: "MY-KEY")
        XCTAssertEqual(config.directory, "/store")
        XCTAssertEqual(config.device, "phone")
        XCTAssertEqual(config.publicKey, "MY-KEY")
        XCTAssertEqual(config.peers, [Peer(name: "pc", fingerprint: "ABCD", publicKey: "PC-KEY", isOwner: true)])
        // The core checks the keys themselves.
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
        settings.dropZoneURL = "sftp://drop@zone.example"
        settings.dropZoneHostKey = "SHA256:abc"
        settings.peers = [PeerSettings(name: "pc", fingerprint: "ABCD", publicKey: "KEY", isOwner: true)]
        settings.passphrasePolicy = .keychainWithBiometrics
        settings.lockAfter = 30
        try store.save(settings)
        XCTAssertEqual(store.load(), settings)

        try store.setSecret("KEY", for: .sshPrivateKey)
        XCTAssertEqual(store.secret(.sshPrivateKey), "KEY")
        try store.setSecret("", for: .sshPrivateKey)
        XCTAssertNil(store.secret(.sshPrivateKey), "an empty secret is no secret")

        try store.setSecret("k", for: .openPGPSecretKey)
        try store.erase()
        XCTAssertEqual(store.load(), PassSettings())
        XCTAssertNil(store.secret(.openPGPSecretKey))
    }

    func testOnlyThePassphraseIsBehindBiometry() {
        XCTAssertEqual(SecretAccount.allCases.filter(\.requiresBiometry), [.passphrase])
    }
}

final class PresentationTests: XCTestCase {
    func testTree() {
        let tree = storeTree(
            ["bank", "Email/work", "Email/Perso", "email", "web/shops/corner", "web/a", "Email", "private/diary"],
            locked: ["private/diary", "web/a"])
        XCTAssertEqual(tree.map(\.id), ["Email/", "private/", "web/", "bank", "Email", "email"])
        let email = tree[0]
        XCTAssertTrue(email.isFolder)
        XCTAssertEqual(email.children.map(\.path), ["Email/Perso", "Email/work"])
        XCTAssertEqual(email.entryCount, 2)
        let web = tree[2]
        XCTAssertEqual(web.children.map(\.id), ["web/shops/", "web/a"])
        XCTAssertEqual(web.children[0].children.map(\.path), ["web/shops/corner"])
        // What this device cannot read is listed, and marked.
        XCTAssertEqual(tree[1].children.map(\.isLocked), [true])
        XCTAssertEqual(web.children.map(\.isLocked), [false, true])
        XCTAssertFalse(tree[3].isLocked)
        // An entry and a folder of the same name are two rows.
        XCTAssertFalse(tree[4].isFolder)
        XCTAssertEqual(tree[4].path, "Email")
        XCTAssertEqual(storeTree([]), [])
    }

    func testSyncStateInWords() {
        func state(store: Bool = true, outstanding: UInt32 = 0, unpublished: Bool = false, waiting: Bool = false) -> SyncState {
            SyncState(
                hasStore: store, outstanding: outstanding, unpublished: unpublished, updateWaiting: waiting,
                ownerPublishedAt: 0, lastSubject: "", lastTime: 0)
        }
        XCTAssertEqual(describe(state()), "In step with the owner.")
        XCTAssertEqual(describe(state(store: false)), "No store yet: sync to receive the owner's.")
        XCTAssertEqual(describe(state(store: false, waiting: true)), "The owner's store has arrived: look at it to take it.")
        XCTAssertEqual(describe(state(waiting: true)), "An update from the owner is waiting.")
        XCTAssertEqual(
            describe(state(outstanding: 2, unpublished: true)),
            "2 change(s) of this device not taken by the owner yet, not published yet.")
    }

    func testWhoCanRead() {
        let pc = Reader(id: "AAAA1111BBBB2222", device: "pc")
        let phone = Reader(id: "phone@example.org", device: "phone")
        let stranger = Reader(id: "0123456789ABCDEF0123456789ABCDEF01234567", device: nil)
        XCTAssertEqual(describe([pc, phone]), "pc, phone")
        XCTAssertEqual(describe([pc, stranger]), "pc, unknown key 01234567")
        XCTAssertEqual(describe([Reader]()), "")

        func access(_ readers: [Reader], encryptedTo: [String], matches: Bool) -> Access {
            Access(gpgId: ".gpg-id", readers: readers, listed: true, encryptedTo: encryptedTo, readable: true, matches: matches)
        }
        XCTAssertEqual(warnings(for: access([pc, phone], encryptedTo: ["pc", "phone"], matches: true)), [])
        XCTAssertEqual(
            warnings(for: access([pc, stranger], encryptedTo: ["pc", "key 1"], matches: true)),
            ["One of its readers is a key this device does not know: it cannot write here."])
        let stale = warnings(for: access([pc], encryptedTo: ["pc", "phone"], matches: false))
        XCTAssertEqual(stale.count, 1)
        XCTAssertTrue(stale[0].hasPrefix("The file is encrypted to pc, phone, not to the readers its folder lists"))
    }

    func testChangeKindsInWords() {
        XCTAssertEqual(describe(ChangeKind.added), "added")
        XCTAssertEqual(describe(ChangeKind.renamed(from: "bank")), "renamed from bank")
    }

    func testFingerprintGrouping() {
        XCTAssertEqual(groupedFingerprint("ABCD1234EF"), "ABCD 1234 EF")
        XCTAssertEqual(groupedFingerprint(""), "")
    }

    func testErrorsFromTheCoreAreReadable() {
        XCTAssertEqual(message(for: PassError.NotFound(name: "bank")), "bank is not in the password store")
        XCTAssertEqual(
            message(for: PassError.ChoiceRequired(names: ["bank", "wifi"])),
            "both sides changed bank, wifi: choose which version to keep")
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
    private var current = Date()

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

/// The model with no computer and no drop zone in sight: keys, devices, the lock.
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
        XCTAssertNil(model.ownPublicKey)

        let unlocked = await model.unlock(passphrase: "x")
        XCTAssertFalse(unlocked)
        XCTAssertEqual(model.errorMessage, "No key on this device.")
        model.errorMessage = nil
        let imported = await model.importSecretKey("not a key", passphrase: "x", policy: .askEachTime)
        XCTAssertFalse(imported)
        XCTAssertNotNil(model.errorMessage)
        XCTAssertNil(fixture.store.secret(.openPGPSecretKey), "nothing is kept of a key that was refused")

        model.errorMessage = nil
        await model.sync()
        XCTAssertEqual(model.errorMessage, "The sync is not set up yet.")
    }

    func testAKeyMadeHereNeverLeavesButItsPublicHalf() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanUp() }
        let model = fixture.model()
        let made = await model.makeKey(passphrase: "correct horse", policy: .askEachTime)
        XCTAssertTrue(made, model.errorMessage ?? "")
        let info = try XCTUnwrap(model.keyInfo)
        XCTAssertEqual(info.userIds, ["pass on phone"])
        XCTAssertEqual(info.fingerprint.count, 40)
        XCTAssertTrue(info.isSecret && info.needsPassphrase)
        XCTAssertTrue(model.isUnlocked, "the passphrase was just typed")
        XCTAssertEqual(model.stage, .needsDropZone)

        // What the owner gets: the public key, and the fingerprint to compare.
        let publicKey = try XCTUnwrap(model.ownPublicKey)
        XCTAssertTrue(publicKey.contains("BEGIN PGP PUBLIC KEY BLOCK"))
        XCTAssertFalse(publicKey.contains("PRIVATE"))
        XCTAssertEqual(try inspectKey(key: publicKey).fingerprint, info.fingerprint)
        // What stays: the secret key, in the keychain, still locked by its passphrase.
        let kept = try XCTUnwrap(fixture.store.secret(.openPGPSecretKey))
        XCTAssertThrowsError(try Identity.unlock(secretKey: kept, passphrase: "wrong"))
        XCTAssertNil(fixture.store.secret(.passphrase), "asked each time: it is nowhere on the phone")

        // A relaunch has the key, and is locked. The right passphrase opens it, and only that.
        let relaunched = fixture.model()
        XCTAssertEqual(relaunched.keyInfo?.fingerprint, info.fingerprint)
        XCTAssertFalse(relaunched.isUnlocked)
        var unlocked = await relaunched.unlockWithBiometrics()
        XCTAssertFalse(unlocked)
        XCTAssertNil(relaunched.errorMessage)
        unlocked = await relaunched.unlock(passphrase: "wrong")
        XCTAssertFalse(unlocked)
        XCTAssertEqual(relaunched.errorMessage, "wrong passphrase")
        unlocked = await relaunched.unlock(passphrase: "correct horse")
        XCTAssertTrue(unlocked)

        // Two minutes without use lock it; use keeps it open; leaving the app locks at once.
        fixture.clock.advance(100)
        relaunched.lockIfExpired()
        XCTAssertTrue(relaunched.isUnlocked)
        relaunched.userDidInteract()
        fixture.clock.advance(100)
        relaunched.lockIfExpired()
        XCTAssertTrue(relaunched.isUnlocked)
        fixture.clock.advance(121)
        relaunched.lockIfExpired()
        XCTAssertFalse(relaunched.isUnlocked)
        _ = await relaunched.unlock(passphrase: "correct horse")
        relaunched.didEnterBackground()
        XCTAssertFalse(relaunched.isUnlocked)
    }

    func testFaceIDUnlocksWithTheKeptPassphrase() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanUp() }
        let model = fixture.model()
        // Even for a key with no passphrase, Face ID then stands between the phone and the key.
        let made = await model.makeKey(passphrase: "", policy: .keychainWithBiometrics)
        XCTAssertTrue(made)
        XCTAssertEqual(model.keyInfo?.needsPassphrase, false)
        XCTAssertTrue(model.canUnlockWithBiometrics)
        model.lock()
        let unlocked = await model.unlockWithBiometrics()
        XCTAssertTrue(unlocked)

        // Going back to typing it removes it from the keychain.
        var settings = model.settings
        settings.passphrasePolicy = .askEachTime
        model.save(settings)
        XCTAssertNil(fixture.store.secret(.passphrase))
        model.lock()
        let again = await model.unlockWithBiometrics()
        XCTAssertFalse(again)
    }

    func testDevicesAreAddedByTheirPublicKeyAndThereIsOneOwner() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanUp() }
        let model = fixture.model()
        _ = await model.makeKey(passphrase: "", policy: .askEachTime)
        let pc = try generateOpenpgpKey(userId: "pc", passphrase: "")
        let tablet = try generateOpenpgpKey(userId: "tablet", passphrase: "")

        XCTAssertFalse(model.addPeer(name: "My PC", publicKey: pc.publicKey, isOwner: true))
        XCTAssertFalse(model.addPeer(name: "phone", publicKey: pc.publicKey, isOwner: true), "this device's own name")
        XCTAssertFalse(model.addPeer(name: "pc", publicKey: pc.secretKey, isOwner: true), "a secret key")
        XCTAssertFalse(model.addPeer(name: "pc", publicKey: "nope", isOwner: true))
        XCTAssertEqual(model.settings.peers, [])
        model.errorMessage = nil

        XCTAssertTrue(model.addPeer(name: "pc", publicKey: pc.publicKey, isOwner: true))
        XCTAssertEqual(model.settings.owner?.fingerprint, pc.fingerprint, "the fingerprint is the key's, to be compared by eye")
        XCTAssertFalse(model.addPeer(name: "pc", publicKey: tablet.publicKey, isOwner: false), "the name is taken")
        XCTAssertFalse(model.addPeer(name: "laptop", publicKey: tablet.publicKey, isOwner: true), "one owner")
        XCTAssertEqual(model.errorMessage, "There is already an owner: pc. Remove it first.")
        model.errorMessage = nil
        XCTAssertTrue(model.addPeer(name: "tablet", publicKey: tablet.publicKey, isOwner: false))
        XCTAssertEqual(fixture.store.load().peers.map(\.name), ["pc", "tablet"])
        XCTAssertNil(model.configurationProblem)

        // The SSH key for the drop zone: made once, its private half stays in the keychain.
        XCTAssertNil(model.deviceKey)
        model.makeDeviceKey()
        let key = try XCTUnwrap(model.deviceKey)
        XCTAssertTrue(key.publicKey.hasPrefix("ssh-ed25519 AAAA"))
        XCTAssertTrue(key.publicKey.hasSuffix(" pass on phone"))
        XCTAssertEqual(key.privateKey, "", "the model does not hand the private half around")
        XCTAssertTrue(try XCTUnwrap(fixture.store.secret(.sshPrivateKey)).contains("OPENSSH PRIVATE KEY"))
        model.makeDeviceKey()
        XCTAssertEqual(model.deviceKey, key, "asking again does not replace the key the drop zone knows")

        XCTAssertEqual(model.stage, .needsDropZone, "still no drop zone")
        var settings = model.settings
        settings.dropZoneURL = "sftp://drop@zone.example:2222"
        settings.dropZoneHostKey = "SHA256:abc"
        model.save(settings)
        XCTAssertEqual(model.stage, .needsStore)
        XCTAssertEqual(model.syncState.map(describe), "No store yet: sync to receive the owner's.")

        model.removePeer(named: "pc")
        XCTAssertEqual(model.stage, .needsDropZone, "no owner: nobody to sync with")
        // Locked, nothing goes to the drop zone: the files are signed with the key.
        _ = model.addPeer(name: "pc", publicKey: pc.publicKey, isOwner: true)
        model.lock()
        await model.sync()
        XCTAssertEqual(model.errorMessage, "Unlock to sync: the files are signed and encrypted with this device's key.")

        model.eraseEverything()
        XCTAssertEqual(model.stage, .needsKey)
        XCTAssertNil(model.deviceKey)
        for account in SecretAccount.allCases {
            XCTAssertNil(fixture.store.secret(account), account.rawValue)
        }
        XCTAssertEqual(fixture.model().stage, .needsKey)
    }
}

/// Swift → UniFFI → Rust → a real drop zone (OpenSSH), on a store made and published by
/// pass.py (`pass sync push`). `swift-test.sh` makes all of it, sets these, and afterwards
/// has pass.py take what was published here (`pass sync fetch`, `log`, `merge`).
@MainActor
final class EndToEndTests: XCTestCase {
    private var fixture: Fixture!
    private var secretKey = ""
    private var sshKey = ""
    private var ownerKey = ""
    private var passphrase = ""
    /// A drop zone holding the owner's store as it was, and one holding it two changes later.
    private var earlier = (url: "", hostKey: "")
    private var later = (url: "", hostKey: "")

    override func setUp() async throws {
        let env = ProcessInfo.processInfo.environment
        guard let fixtures = env["PASS_E2E_FIXTURES"], let passphrase = env["PASS_E2E_PASSPHRASE"],
              let earlierURL = env["PASS_E2E_EARLIER_URL"], let earlierKey = env["PASS_E2E_EARLIER_HOST_KEY"],
              let laterURL = env["PASS_E2E_LATER_URL"], let laterKey = env["PASS_E2E_LATER_HOST_KEY"] else {
            throw XCTSkip("no fixtures: run `make swift-test`")
        }
        let directory = URL(fileURLWithPath: fixtures)
        secretKey = try String(contentsOf: directory.appendingPathComponent("phone-secret.asc"), encoding: .utf8)
        sshKey = try String(contentsOf: directory.appendingPathComponent("phone.ssh"), encoding: .utf8)
        ownerKey = try String(contentsOf: directory.appendingPathComponent("pc-public.asc"), encoding: .utf8)
        self.passphrase = passphrase
        earlier = (earlierURL, earlierKey)
        later = (laterURL, laterKey)
        fixture = try Fixture()
    }

    override func tearDown() async throws {
        fixture?.cleanUp()
    }

    private func point(_ model: AppModel, at zone: (url: String, hostKey: String)) {
        var settings = model.settings
        settings.dropZoneURL = zone.url
        settings.dropZoneHostKey = zone.hostKey
        model.save(settings)
    }

    /// Imports the key, names the owner, points at a drop zone: a model ready for its first sync, unlocked.
    private func configuredModel(zone: (url: String, hostKey: String)) async throws -> AppModel {
        try fixture.store.setSecret(sshKey, for: .sshPrivateKey)
        let model = fixture.model()
        let imported = await model.importSecretKey(secretKey, passphrase: passphrase, policy: .askEachTime)
        XCTAssertTrue(imported, model.errorMessage ?? "")
        XCTAssertEqual(model.stage, .needsDropZone)
        XCTAssertTrue(model.addPeer(name: "pc", publicKey: ownerKey, isOwner: true), model.errorMessage ?? "")
        point(model, at: zone)
        XCTAssertNil(model.configurationProblem)
        XCTAssertEqual(model.stage, .needsStore)
        return model
    }

    /// The same, with the owner's store received, looked at and accepted.
    private func readyModel(zone: (url: String, hostKey: String)) async throws -> AppModel {
        let model = try await configuredModel(zone: zone)
        await model.sync()
        XCTAssertNil(model.errorMessage)
        let accepted = await model.acceptUpdate()
        XCTAssertTrue(accepted, model.errorMessage ?? "")
        XCTAssertEqual(model.stage, .ready)
        return model
    }

    func testTheOwnersStoreIsShownThenTaken() async throws {
        let model = try await configuredModel(zone: earlier)
        await model.sync()
        XCTAssertNil(model.errorMessage)
        XCTAssertEqual(model.syncMessage, "An update from the owner is waiting.")
        XCTAssertEqual(model.stage, .needsStore, "received is not taken")
        XCTAssertEqual(model.entries, [])
        XCTAssertEqual(model.syncState.map(describe), "The owner's store has arrived: look at it to take it.")
        XCTAssertEqual(model.ownerPublishedAgo?.hasPrefix("published "), true)

        // What it would do: the entries, who can read which folder, what is not an entry.
        let update = try XCTUnwrap(model.pendingUpdate)
        XCTAssertEqual(update.entries.map(\.name), ["Email/work", "bank", "private/diary", "wifi"])
        XCTAssertEqual(update.entries.map(\.kind), [.added, .added, .added, .added])
        XCTAssertEqual(update.entries[0].readersAfter, ["pc", "phone"])
        XCTAssertEqual(update.entries[2].readersAfter, ["pc"])
        XCTAssertEqual(update.recipientChanges.map(\.path), [".gpg-id", "private/.gpg-id"])
        XCTAssertEqual(update.recipientChanges.map { describe($0.after) }, ["pc, phone", "pc"])
        XCTAssertEqual(update.otherFiles, [".gitattributes"])
        XCTAssertEqual(update.outstanding, 0)
        XCTAssertEqual(model.undecided, [])
        // An entry can be read before the update is taken; one that is not for this device says so.
        let bank = await model.versions(of: "bank")
        XCTAssertEqual(bank?.before, .absent)
        XCTAssertEqual(bank?.theirs, .content(text: "hunter2\nlogin: alice\notpauth://totp/x?secret=JBSWY3DPEHPK3PXP\n"))
        let diary = await model.versions(of: "private/diary")
        guard case .unreadable = diary?.theirs else { return XCTFail("the diary is not for the phone") }
        // It is still there after a restart of the app, and still not applied.
        XCTAssertEqual(fixture.model().stage, .needsStore)

        let accepted = await model.acceptUpdate()
        XCTAssertTrue(accepted, model.errorMessage ?? "")
        XCTAssertEqual(model.stage, .ready)
        XCTAssertNil(model.pendingUpdate)
        XCTAssertEqual(model.entries, ["Email/work", "bank", "private/diary", "wifi"])
        XCTAssertEqual(model.syncState.map(describe), "In step with the owner.")
        XCTAssertEqual(model.syncMessage, "Published this device's changes. Nothing new from the owner.")

        // Who can read what, folder by folder; what this device cannot read is listed, locked.
        XCTAssertEqual(model.locked, ["private/diary"])
        XCTAssertEqual(model.tree.map(\.id), ["Email/", "private/", "bank", "wifi"])
        XCTAssertEqual(model.tree[1].children.map(\.isLocked), [true])
        let root = await model.access(toFolder: "")
        XCTAssertEqual(root.map { describe($0.readers) }, "pc, phone")
        let privateFolder = await model.access(toFolder: "private")
        XCTAssertEqual(privateFolder.map { describe($0.readers) }, "pc")
        XCTAssertEqual(privateFolder?.readable, false)
        let bankAccess = try await XCTUnwrapAsync(await model.access(toEntry: "bank"))
        XCTAssertEqual(bankAccess.encryptedTo, ["pc", "phone"])
        XCTAssertTrue(bankAccess.readable && bankAccess.matches)
        XCTAssertEqual(warnings(for: bankAccess), [])
        let diaryAccess = try await XCTUnwrapAsync(await model.access(toEntry: "private/diary"))
        XCTAssertFalse(diaryAccess.readable)
        let locked = await model.show("private/diary")
        XCTAssertNil(locked)
        XCTAssertEqual(model.errorMessage, "this entry is not encrypted to the key on this device")
        model.errorMessage = nil

        // Reading, searching, copying.
        model.searchText = " WORK  em "
        XCTAssertEqual(model.searchResults, ["Email/work"])
        let shown = await model.show("bank")
        let entry = try XCTUnwrap(shown)
        XCTAssertEqual(entry.password, "hunter2")
        XCTAssertEqual(entry.fields, [EntryField(name: "login", value: "alice")])
        XCTAssertEqual(model.oneTimeCode(for: entry.otpUris[0])?.code.count, 6)
        model.copy(entry.password)
        let copied = try XCTUnwrap(fixture.pasteboard.last)
        XCTAssertEqual(copied.text, "hunter2")
        XCTAssertEqual(copied.expiresAt, fixture.clock.now.addingTimeInterval(45))
        // The timeout is checked before decrypting, not only by the timer.
        fixture.clock.advance(121)
        let late = await model.show("bank")
        XCTAssertNil(late)
        XCTAssertEqual(model.errorMessage, "The store is locked.")
        XCTAssertEqual(model.entries.count, 4, "the names stay: listing needs no key")
    }

    func testChangesHereArePublishedAndCanBeDropped() async throws {
        let model = try await readyModel(zone: earlier)
        let added = await model.add("phone/added", content: "from-phone")
        XCTAssertTrue(added, model.errorMessage ?? "")
        XCTAssertEqual(model.syncMessage, "Published this device's changes. Nothing new from the owner.")
        XCTAssertEqual(model.syncState.map(describe), "1 change(s) of this device not taken by the owner yet.")
        let generated = await model.generate("phone/generated", length: 24, symbols: false)
        XCTAssertEqual(generated?.count, 24)
        let duplicate = await model.add("phone/added", content: "again")
        XCTAssertFalse(duplicate)
        XCTAssertEqual(model.errorMessage, "an entry already exists for phone/added")
        model.errorMessage = nil
        // A folder this device cannot read: it can still write there, for those who can.
        let intruding = await model.add("private/mine", content: "x")
        XCTAssertTrue(intruding, "its one reader is a device, whose key is here")
        XCTAssertEqual(model.locked, ["private/diary", "private/mine"])
        XCTAssertEqual(model.syncState?.outstanding, 3)

        await model.discardOutstanding()
        XCTAssertNil(model.errorMessage)
        XCTAssertEqual(model.entries, ["Email/work", "bank", "private/diary", "wifi"])
        XCTAssertEqual(model.syncState.map(describe), "In step with the owner.")
        XCTAssertEqual(model.syncMessage, "Dropped 3 change(s) of this device.")
    }

    /// What `swift-test.sh` then has pass.py look at and take: `pass sync fetch`, `log`, `merge`.
    func testAnUpdateIsMergedWithAChoiceThenPublished() async throws {
        // Set up from the owner's store as it was, then changed here while the owner went on.
        let model = try await readyModel(zone: earlier)
        _ = await model.save("bank", content: "phone-version\n")
        _ = await model.add("swift/added", content: "from-swift\nlogin: swift")
        _ = await model.rename("wifi", to: "home/wifi")
        XCTAssertEqual(model.syncState?.outstanding, 3)

        point(model, at: later)
        await model.sync()
        XCTAssertNil(model.errorMessage)
        let update = try XCTUnwrap(model.pendingUpdate)
        XCTAssertEqual(update.outstanding, 3)
        XCTAssertEqual(update.entries.map(\.name), ["bank", "later"])
        XCTAssertEqual(update.entries.map(\.kind), [.changed, .added])
        XCTAssertEqual(update.entries.map(\.conflict), [true, false])
        XCTAssertEqual(update.commits.map(\.subject), [
            "Add given password for later to store.", "Add given password for bank to store.",
        ])
        XCTAssertEqual(model.entries.contains("later"), false, "nothing is applied yet")

        // Both changed bank: nothing is taken until one version is chosen, having seen both.
        XCTAssertEqual(model.undecided, ["bank"])
        var accepted = await model.acceptUpdate()
        XCTAssertFalse(accepted)
        XCTAssertEqual(model.errorMessage, "Choose a version for: bank.")
        model.errorMessage = nil
        let versions = await model.versions(of: "bank")
        XCTAssertEqual(versions?.mine, .content(text: "phone-version\n"))
        XCTAssertEqual(versions?.theirs, .content(text: "pc-version\n"))
        model.choose(.mine, for: "bank")
        XCTAssertEqual(model.undecided, [])
        accepted = await model.acceptUpdate()
        XCTAssertTrue(accepted, model.errorMessage ?? "")

        XCTAssertNil(model.pendingUpdate)
        XCTAssertEqual(model.choices, [:])
        XCTAssertEqual(model.entries, ["Email/work", "bank", "home/wifi", "later", "private/diary", "swift/added"])
        let bank = await model.show("bank")
        XCTAssertEqual(bank?.password, "phone-version")
        let later = await model.show("later")
        XCTAssertEqual(later?.password, "from-pc")
        XCTAssertEqual(model.syncState?.outstanding, 4, "its three commits and the merge")
        XCTAssertEqual(model.syncState?.unpublished, false, "accepted, then published")
        XCTAssertEqual(model.syncMessage, "Published this device's changes. Nothing new from the owner.")
    }
}

/// `XCTUnwrap` for a value that was awaited.
func XCTUnwrapAsync<T>(_ value: T?, file: StaticString = #filePath, line: UInt = #line) async throws -> T {
    try XCTUnwrap(value, file: file, line: line)
}
