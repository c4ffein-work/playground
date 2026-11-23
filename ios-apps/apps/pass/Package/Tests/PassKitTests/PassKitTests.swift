import Foundation
import XCTest
@testable import PassKit

final class SettingsTests: XCTestCase {
    func testTheSyncNeedsADropZoneAndItsPinnedKey() {
        var settings = PassSettings()
        XCTAssertFalse(settings.isSyncComplete)
        settings.dropZoneURL = " sftp://drop@zone.example:2222\n"
        XCTAssertFalse(settings.isSyncComplete, "nothing is sent to a server whose key is not pinned")
        settings.dropZoneHostKey = "SHA256:abc"
        XCTAssertTrue(settings.isSyncComplete, "a device alone publishes for those to come")
        settings.peers = [
            PeerSettings(name: "tablet", fingerprint: "AB", publicKey: "KEY"),
            PeerSettings(name: "pc", fingerprint: "CD", publicKey: "KEY"),
        ]
        XCTAssertEqual(settings.deviceNames, ["pc", "phone", "tablet"])

        XCTAssertNil(settings.dropZone(sshPrivateKey: nil), "no SSH key yet")
        let zone = settings.dropZone(sshPrivateKey: "KEY")
        XCTAssertEqual(zone?.url, "sftp://drop@zone.example:2222")
        XCTAssertEqual(zone?.hostKey, "SHA256:abc")

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
        settings.peers = [PeerSettings(name: "pc", fingerprint: "ABCD", publicKey: "PC-KEY")]
        let config = settings.storeConfig(directory: URL(fileURLWithPath: "/store"), ownPublicKey: "MY-KEY")
        XCTAssertEqual(config.directory, "/store")
        XCTAssertEqual(config.device, "phone")
        XCTAssertEqual(config.publicKey, "MY-KEY")
        XCTAssertEqual(config.peers, [Peer(name: "pc", fingerprint: "ABCD", publicKey: "PC-KEY")])
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
        settings.peers = [PeerSettings(name: "pc", fingerprint: "ABCD", publicKey: "KEY")]
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
        func peer(_ name: String, _ toTake: UInt32) -> PeerState {
            PeerState(device: name, received: true, toTake: toTake, publishedAt: 0)
        }
        func state(store: Bool = true, unpublished: Bool = false, _ peers: [PeerState] = []) -> SyncState {
            SyncState(hasStore: store, unpublished: unpublished, peers: peers, lastSubject: "", lastTime: 0)
        }
        XCTAssertEqual(describe(state()), "Nothing to take.")
        XCTAssertEqual(describe(state(store: false)), "No store yet: sync to receive one, or make one here.")
        XCTAssertEqual(
            describe(state(store: false, [peer("pc", 0), peer("tablet", 3)])),
            "A store has arrived from tablet: look at it to take it.")
        XCTAssertEqual(describe(state([peer("pc", 2)])), "2 to take from pc.")
        XCTAssertEqual(
            describe(state(unpublished: true, [peer("pc", 2), peer("tablet", 1)])),
            "2 to take from pc, 1 to take from tablet, not published yet.")
        XCTAssertEqual(describe(state(unpublished: true)), "Not published yet.")
    }

    func testAVisitToTheDropZoneInWords() {
        func sync(_ name: String, _ outcome: FetchOutcome, toTake: UInt32 = 0) -> PeerSync {
            PeerSync(outcome: outcome, state: PeerState(device: name, received: true, toTake: toTake, publishedAt: 0))
        }
        XCTAssertEqual(describe(sync("pc", .received, toTake: 2)), "pc: 2 to take")
        XCTAssertEqual(describe(sync("pc", .unchanged)), "pc: nothing to take")
        XCTAssertEqual(describe(sync("pc", .nothingPublished)), "pc: nothing published")
        XCTAssertEqual(describe(sync("pc", .refused(reason: "it is not signed"))), "pc: refused, it is not signed")
        let report = SyncReport(
            published: true, publishRefused: nil,
            peers: [sync("pc", .received, toTake: 1), sync("tablet", .stale(publishedAt: 1))])
        XCTAssertEqual(
            describe(report),
            "Published. pc: 1 to take. tablet: an older file than the one already received was served, ignored.")
        let refused = SyncReport(published: false, publishRefused: "the store holds README", peers: [])
        XCTAssertEqual(describe(refused), "Not published: the store holds README.")
    }

    func testAReviewInWords() {
        let lines: [ReviewLine] = [
            .added(name: "bank"), .moved(from: "a", to: "b/a"), .readers(folder: "work/", devices: ["pc", "phone"], inherited: false),
            .readers(folder: "old/", devices: [], inherited: true), .stale(name: "work/w1"), .conflict(name: "bank"),
            .conflict(name: "work/"),
        ]
        XCTAssertEqual(lines.map { reviewLineText(line: $0) }, [
            "added bank", "moved a -> b/a", "readers work/: pc phone", "readers old/: inherited", "stale work/w1",
            "conflict bank", "conflict work/",
        ])
        let update = PendingUpdate(
            device: "pc", commit: "c", publishedAt: 0, kind: .merge, commits: [], commitCount: 0, lines: lines, reencrypts: 1)
        XCTAssertEqual(conflicts(in: update), ["bank", "work/"])
        XCTAssertEqual([TakeKind.adopt, .forward, .merge].map(describe), ["Take this store", "Take", "Merge"])
    }

    func testWhoCanRead() {
        let pc = Reader(id: "AAAA1111BBBB2222", device: "pc")
        let phone = Reader(id: "phone@example.org", device: "phone")
        let stranger = Reader(id: "0123456789ABCDEF0123456789ABCDEF01234567", device: nil)
        XCTAssertEqual(describe([pc, phone]), "pc, phone")
        XCTAssertEqual(describe([pc, stranger]), "pc, unknown key 01234567")
        XCTAssertEqual(describe([Reader]()), "")

        func access(_ readers: [Reader], encryptedTo: [String], matches: Bool) -> Access {
            Access(gpgId: ".gpg-id", readers: readers, listed: true, encryptedTo: encryptedTo, readable: true, stale: !matches)
        }
        XCTAssertEqual(warnings(for: access([pc, phone], encryptedTo: ["pc", "phone"], matches: true)), [])
        XCTAssertEqual(
            warnings(for: access([pc, stranger], encryptedTo: ["pc", "key 1"], matches: true)),
            ["One of its readers is a key this device does not know: it cannot write here."])
        let stale = warnings(for: access([pc], encryptedTo: ["pc", "phone"], matches: false))
        XCTAssertEqual(stale.count, 1)
        XCTAssertTrue(stale[0].hasPrefix("The file is encrypted to pc, phone, not to the readers its folder names"))
    }

    func testFingerprintGrouping() {
        XCTAssertEqual(groupedFingerprint("ABCD1234EF"), "ABCD 1234 EF")
        XCTAssertEqual(groupedFingerprint(""), "")
    }

    func testErrorsFromTheCoreAreReadable() {
        XCTAssertEqual(message(for: PassError.NotFound(name: "bank")), "bank is not in the password store")
        XCTAssertTrue(message(for: PassError.ChoiceRequired(names: ["bank", "work/"])).contains("bank, work/"))
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
        XCTAssertEqual(info.userIds, ["phone"], "the name the other devices know it by")
        XCTAssertEqual(info.fingerprint.count, 40)
        XCTAssertTrue(info.isSecret && info.needsPassphrase)
        XCTAssertTrue(model.isUnlocked, "the passphrase was just typed")
        XCTAssertEqual(model.stage, .needsDropZone)

        // What the other devices get: the public key, and the fingerprint to compare.
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

    func testDevicesAreAddedByTheirPublicKey() async throws {
        let fixture = try Fixture()
        defer { fixture.cleanUp() }
        let model = fixture.model()
        _ = await model.makeKey(passphrase: "", policy: .askEachTime)
        let pc = try generateOpenpgpKey(userId: "pc", passphrase: "")
        let tablet = try generateOpenpgpKey(userId: "tablet", passphrase: "")

        XCTAssertFalse(model.addPeer(name: "My PC", publicKey: pc.publicKey))
        XCTAssertFalse(model.addPeer(name: "phone", publicKey: pc.publicKey), "this device's own name")
        XCTAssertFalse(model.addPeer(name: "pc", publicKey: pc.secretKey), "a secret key")
        XCTAssertFalse(model.addPeer(name: "pc", publicKey: "nope"))
        XCTAssertFalse(model.addPeer(name: "pc", publicKey: try XCTUnwrap(model.ownPublicKey)), "this device's own key")
        XCTAssertEqual(model.settings.peers, [])
        model.errorMessage = nil

        XCTAssertTrue(model.addPeer(name: "pc", publicKey: pc.publicKey))
        XCTAssertEqual(model.settings.peers[0].fingerprint, pc.fingerprint, "the fingerprint is the key's, to be compared by eye")
        XCTAssertFalse(model.addPeer(name: "pc", publicKey: tablet.publicKey), "the name is taken")
        XCTAssertFalse(model.addPeer(name: "laptop", publicKey: pc.publicKey), "one key, one device")
        XCTAssertEqual(model.errorMessage, "This key is already a device's.")
        model.errorMessage = nil
        XCTAssertTrue(model.addPeer(name: "tablet", publicKey: tablet.publicKey))
        XCTAssertEqual(fixture.store.load().peers.map(\.name), ["pc", "tablet"])
        XCTAssertEqual(model.settings.deviceNames, ["pc", "phone", "tablet"])
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
        // Nobody listens there: what is made here stays to be published.
        settings.dropZoneURL = "sftp://drop@127.0.0.1:9"
        settings.dropZoneHostKey = "SHA256:abc"
        model.save(settings)
        XCTAssertEqual(model.stage, .needsStore)
        XCTAssertEqual(model.syncState.map(describe), "No store yet: sync to receive one, or make one here.")
        XCTAssertEqual(model.peers.map(\.device), ["pc", "tablet"])
        XCTAssertEqual(model.peers.map(model.standing), ["nothing received", "nothing received"])
        model.removePeer(named: "tablet")
        XCTAssertEqual(model.stage, .needsStore, "the others can come later")

        // No device has a store: this one makes it, for itself and the computer.
        var created = await model.createStore(readers: ["nobody"])
        XCTAssertFalse(created)
        model.errorMessage = nil
        created = await model.createStore(readers: ["phone", "pc"])
        XCTAssertTrue(created, model.errorMessage ?? "")
        XCTAssertEqual(model.stage, .ready)
        XCTAssertEqual(model.syncMessage?.hasPrefix("Not published yet: "), true)
        XCTAssertEqual(model.syncState.map(describe), "Not published yet.")
        let readers = await model.ownReaders(ofFolder: "")
        XCTAssertEqual(readers, ["pc", "phone"])
        let added = await model.add("bank", content: "hunter2")
        XCTAssertTrue(added, model.errorMessage ?? "")
        let access = await model.access(toEntry: "bank")
        XCTAssertEqual(access?.encryptedTo, ["pc", "phone"])
        XCTAssertNil(model.errorMessage, "the network is no alert when a change is made")
        // Asked for, the sync says what is wrong.
        await model.sync()
        XCTAssertNotNil(model.errorMessage)
        model.errorMessage = nil
        // Locked, nothing goes to the drop zone: the files are signed with the key.
        model.lock()
        await model.sync()
        XCTAssertEqual(model.errorMessage, "Unlock to sync: the files are signed and encrypted with this device's key.")
        XCTAssertEqual(fixture.model().entries, [], "a relaunch lists nothing until it is refreshed")

        model.eraseEverything()
        XCTAssertEqual(model.stage, .needsKey)
        XCTAssertNil(model.deviceKey)
        XCTAssertEqual(model.entries, [])
        for account in SecretAccount.allCases {
            XCTAssertNil(fixture.store.secret(account), account.rawValue)
        }
        XCTAssertEqual(fixture.model().stage, .needsKey)
    }
}

/// Swift → UniFFI → Rust → a real drop zone (OpenSSH), with another device that is pass.py
/// (`pass sync push`). `swift-test.sh` makes all of it, sets these, and afterwards has pass.py
/// take what was published here (`pass sync fetch`, `log`, `merge`).
@MainActor
final class EndToEndTests: XCTestCase {
    private var fixture: Fixture!
    private var secretKey = ""
    private var sshKey = ""
    private var computerKey = ""
    private var passphrase = ""
    /// A drop zone holding the computer's store as it was, and one holding it two changes later.
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
        computerKey = try String(contentsOf: directory.appendingPathComponent("pc-public.asc"), encoding: .utf8)
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

    /// Imports the key, names the computer, points at a drop zone: a model ready for its first sync, unlocked.
    private func configuredModel(zone: (url: String, hostKey: String)) async throws -> AppModel {
        try fixture.store.setSecret(sshKey, for: .sshPrivateKey)
        let model = fixture.model()
        let imported = await model.importSecretKey(secretKey, passphrase: passphrase, policy: .askEachTime)
        XCTAssertTrue(imported, model.errorMessage ?? "")
        XCTAssertEqual(model.stage, .needsDropZone)
        XCTAssertTrue(model.addPeer(name: "pc", publicKey: computerKey), model.errorMessage ?? "")
        point(model, at: zone)
        XCTAssertNil(model.configurationProblem)
        XCTAssertEqual(model.stage, .needsStore)
        return model
    }

    /// The same, with the computer's store received, looked at and taken.
    private func readyModel(zone: (url: String, hostKey: String)) async throws -> AppModel {
        let model = try await configuredModel(zone: zone)
        await model.sync()
        XCTAssertNil(model.errorMessage)
        let update = await model.openReview(of: "pc")
        XCTAssertEqual(update?.kind, .adopt)
        let taken = await model.take()
        XCTAssertTrue(taken, model.errorMessage ?? "")
        XCTAssertEqual(model.stage, .ready)
        return model
    }

    func testAnotherDevicesStoreIsShownThenTaken() async throws {
        let model = try await configuredModel(zone: earlier)
        await model.sync()
        XCTAssertNil(model.errorMessage)
        let message = try XCTUnwrap(model.syncMessage)
        XCTAssertTrue(message.hasPrefix("pc: ") && message.hasSuffix(" to take."), message)
        XCTAssertEqual(model.stage, .needsStore, "received is not taken")
        XCTAssertEqual(model.entries, [])
        XCTAssertEqual(model.syncState.map(describe), "A store has arrived from pc: look at it to take it.")
        let peer = try XCTUnwrap(model.peers.first)
        XCTAssertEqual(peer.device, "pc")
        XCTAssertTrue(peer.received && peer.toTake > 0)
        XCTAssertEqual(model.standing(of: peer), "\(peer.toTake) to take")
        XCTAssertEqual(model.publishedAgo(peer)?.hasPrefix("published "), true)

        // What it would do, in the words of SYNC.md: the entries, and who reads which folder.
        let opened = await model.openReview(of: "pc")
        let update = try XCTUnwrap(opened)
        XCTAssertEqual(update.kind, .adopt)
        XCTAssertEqual(update.commitCount, peer.toTake)
        XCTAssertEqual(model.reviewLines.sorted(), [
            "added Email/work", "added bank", "added private/diary", "added wifi",
            "readers /: pc phone", "readers private/: pc",
        ])
        XCTAssertEqual(update.reencrypts, 0)
        XCTAssertEqual(model.undecided, [])
        // An entry can be read before the update is taken; one that is not for this device says so.
        let bank = await model.versions(of: "bank")
        XCTAssertEqual(bank?.base, .absent)
        XCTAssertEqual(bank?.theirs, .content(text: "hunter2\nlogin: alice\notpauth://totp/x?secret=JBSWY3DPEHPK3PXP\n"))
        let diary = await model.versions(of: "private/diary")
        guard case .unreadable = diary?.theirs else { return XCTFail("the diary is not for the phone") }
        // It is still there after a restart of the app, and still not applied.
        XCTAssertEqual(fixture.model().stage, .needsStore)
        XCTAssertEqual(fixture.model().peers.first?.toTake, peer.toTake)
        // Left, it stays to take.
        model.closeReview()
        XCTAssertNil(model.review)
        let nothing = await model.take()
        XCTAssertFalse(nothing)
        _ = await model.openReview(of: "pc")

        let taken = await model.take()
        XCTAssertTrue(taken, model.errorMessage ?? "")
        XCTAssertEqual(model.stage, .ready)
        XCTAssertNil(model.review)
        XCTAssertEqual(model.entries, ["Email/work", "bank", "private/diary", "wifi"])
        XCTAssertEqual(model.syncState.map(describe), "Nothing to take.")
        XCTAssertEqual(model.syncMessage, "Published. pc: nothing to take.")
        XCTAssertEqual(model.peers.map(model.standing), ["nothing to take"])
        let none = await model.openReview(of: "pc")
        XCTAssertNil(none)
        XCTAssertNil(model.errorMessage)

        // Who can read what, folder by folder; what this device cannot read is listed, locked.
        XCTAssertEqual(model.locked, ["private/diary"])
        XCTAssertEqual(model.stale, [])
        XCTAssertEqual(model.tree.map(\.id), ["Email/", "private/", "bank", "wifi"])
        XCTAssertEqual(model.tree[1].children.map(\.isLocked), [true])
        let root = await model.access(toFolder: "")
        XCTAssertEqual(root.map { describe($0.readers) }, "pc, phone")
        let privateFolder = await model.access(toFolder: "private")
        XCTAssertEqual(privateFolder.map { describe($0.readers) }, "pc")
        XCTAssertEqual(privateFolder?.readable, false)
        let bankAccess = try await XCTUnwrapAsync(await model.access(toEntry: "bank"))
        XCTAssertEqual(bankAccess.encryptedTo, ["pc", "phone"])
        XCTAssertTrue(bankAccess.readable && !bankAccess.stale)
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
        XCTAssertEqual(model.syncMessage, "Published. pc: nothing to take.")
        XCTAssertEqual(model.syncState?.unpublished, false)
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
        let moved = await model.renameFolder("phone", to: "mobile")
        XCTAssertTrue(moved, model.errorMessage ?? "")
        XCTAssertEqual(model.entries.filter { $0.hasPrefix("mobile/") }, ["mobile/added", "mobile/generated"])

        // Back to where the computer stands: everything made here since is given up.
        await model.drop(to: "pc")
        XCTAssertNil(model.errorMessage)
        XCTAssertEqual(model.entries, ["Email/work", "bank", "private/diary", "wifi"])
        XCTAssertEqual(model.syncState.map(describe), "Nothing to take.")
        XCTAssertEqual(model.syncMessage, "Dropped 4 change(s) of this device.")
    }

    func testTheReadersOfAFolder() async throws {
        let model = try await readyModel(zone: earlier)
        XCTAssertEqual(model.settings.deviceNames, ["pc", "phone"])
        var own = await model.ownReaders(ofFolder: "")
        XCTAssertEqual(own, ["pc", "phone"])
        own = await model.ownReaders(ofFolder: "Email")
        XCTAssertNil(own, "it has those of the folder above")

        // A folder for the phone alone: what it reads there is encrypted again, at once.
        _ = await model.add("mine/plan", content: "secret")
        var changed = await model.setReaders(ofFolder: "mine", to: ["phone"])
        XCTAssertTrue(changed, model.errorMessage ?? "")
        own = await model.ownReaders(ofFolder: "mine")
        XCTAssertEqual(own, ["phone"])
        var access = try await XCTUnwrapAsync(await model.access(toEntry: "mine/plan"))
        XCTAssertEqual(access.encryptedTo, ["phone"])
        XCTAssertEqual(model.stale, [])
        XCTAssertEqual(model.syncState?.unpublished, false, "changed, then published")
        // And back to the readers of the folder above.
        changed = await model.setReaders(ofFolder: "mine", to: [])
        XCTAssertTrue(changed, model.errorMessage ?? "")
        own = await model.ownReaders(ofFolder: "mine")
        XCTAssertNil(own)
        access = try await XCTUnwrapAsync(await model.access(toEntry: "mine/plan"))
        XCTAssertEqual(access.encryptedTo, ["pc", "phone"])

        // Naming itself where it does not read is asking: the entries stay as they are, and are told stale.
        changed = await model.setReaders(ofFolder: "private", to: ["phone", "pc"])
        XCTAssertTrue(changed, model.errorMessage ?? "")
        XCTAssertEqual(model.stale, ["private/diary"])
        XCTAssertEqual(model.locked, ["private/diary"])
        access = try await XCTUnwrapAsync(await model.access(toEntry: "private/diary"))
        XCTAssertTrue(access.listed && access.stale && !access.readable)
        XCTAssertEqual(warnings(for: access).count, 1)
        let diary = await model.show("private/diary")
        XCTAssertNil(diary)
        model.errorMessage = nil

        // Only devices, and the top of the store always names its own.
        changed = await model.setReaders(ofFolder: "mine", to: ["nobody"])
        XCTAssertFalse(changed)
        model.errorMessage = nil
        changed = await model.setReaders(ofFolder: "", to: [])
        XCTAssertFalse(changed)
        XCTAssertNotNil(model.errorMessage)
    }

    /// What `swift-test.sh` then has pass.py look at and take: `pass sync fetch`, `log`, `merge`.
    func testAnUpdateIsMergedWithAChoiceThenPublished() async throws {
        // Set up from the computer's store as it was, then changed here while the computer went on.
        let model = try await readyModel(zone: earlier)
        _ = await model.save("bank", content: "phone-version\n")
        _ = await model.add("swift/added", content: "from-swift\nlogin: swift")
        _ = await model.rename("wifi", to: "home/wifi")

        point(model, at: later)
        await model.sync()
        XCTAssertNil(model.errorMessage)
        XCTAssertEqual(model.syncMessage, "pc: 2 to take.")
        XCTAssertEqual(model.syncState.map(describe), "2 to take from pc.")
        let opened = await model.openReview(of: "pc")
        let update = try XCTUnwrap(opened)
        XCTAssertEqual(update.kind, .merge)
        XCTAssertEqual(model.reviewLines.sorted(), ["added later", "changed bank", "conflict bank"])
        XCTAssertEqual(update.commits.map(\.subject), [
            "Add given password for later to store.", "Add given password for bank to store.",
        ])
        XCTAssertEqual(update.commits.map(\.signature), [.valid(device: "pc"), .valid(device: "pc")], "shown, never required")
        XCTAssertEqual(model.entries.contains("later"), false, "nothing is applied yet")

        // Both changed bank: nothing is taken until one side is chosen, having seen both.
        XCTAssertEqual(model.undecided, ["bank"])
        var taken = await model.take()
        XCTAssertFalse(taken)
        XCTAssertEqual(model.errorMessage, "Choose a side for: bank.")
        model.errorMessage = nil
        let versions = await model.versions(of: "bank")
        XCTAssertEqual(versions?.mine, .content(text: "phone-version\n"))
        XCTAssertEqual(versions?.theirs, .content(text: "pc-version\n"))
        model.choose(.mine, for: "bank")
        XCTAssertEqual(model.undecided, [])
        taken = await model.take()
        XCTAssertTrue(taken, model.errorMessage ?? "")

        XCTAssertNil(model.review)
        XCTAssertEqual(model.choices, [:])
        XCTAssertEqual(model.entries, ["Email/work", "bank", "home/wifi", "later", "private/diary", "swift/added"])
        let bank = await model.show("bank")
        XCTAssertEqual(bank?.password, "phone-version")
        let later = await model.show("later")
        XCTAssertEqual(later?.password, "from-pc")
        XCTAssertEqual(model.syncState?.unpublished, false, "taken, then published")
        XCTAssertEqual(model.syncMessage, "Published. pc: nothing to take.")
    }
}

/// `XCTUnwrap` for a value that was awaited.
func XCTUnwrapAsync<T>(_ value: T?, file: StaticString = #filePath, line: UInt = #line) async throws -> T {
    try XCTUnwrap(value, file: file, line: line)
}
