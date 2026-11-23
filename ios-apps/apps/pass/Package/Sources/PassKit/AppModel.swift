import Foundation
import Observation

/// Where copied secrets go. On the phone: the general pasteboard, for this device
/// only (no Universal Clipboard) and until the expiry (see `App/Pasteboard.swift`).
public protocol SecretPasteboard: Sendable {
    func copy(_ text: String, expiresAt: Date)
}

/// What is missing before the store can be used, in the order the setup asks for it.
public enum SetupStage: Equatable, Sendable {
    /// No OpenPGP key: make one here, or import one.
    case needsKey
    /// The sync is not fully described: this device's name, the drop zone, its pinned host key, this device's SSH key.
    case needsDropZone
    /// Everything is there but the store itself: sync, look at what another device published and
    /// take it; or make the store here.
    case needsStore
    case ready
}

/// Everything the views show, and everything they can ask for. No view talks to the core itself.
///
/// The unlocked key lives here and nowhere else: `lock()` drops it, and with it the
/// only thing that can read an entry. Nothing decrypted is kept by the model.
@MainActor
@Observable
public final class AppModel {
    public private(set) var settings: PassSettings
    /// This device's OpenPGP key, described. `nil` when there is none.
    public private(set) var keyInfo: KeyInfo?
    /// Its public half, armored: what every other device is given.
    public private(set) var ownPublicKey: String?
    /// The public half of this device's SSH key: what the drop zone is given.
    public private(set) var deviceKey: DeviceKey?

    /// Every entry of the store, sorted.
    public private(set) var entries: [String] = []
    /// The entries this device cannot decrypt: listed, by name only.
    public private(set) var locked: Set<String> = []
    /// What the search field holds. Each word narrows the list.
    public var searchText = ""

    public private(set) var isUnlocked = false
    public private(set) var syncState: SyncState?
    public private(set) var isSyncing = false
    /// What the last sync did, in a sentence.
    public private(set) var syncMessage: String?
    /// How each peer answered at the last visit to the drop zone, by name: a refusal stays told.
    public private(set) var outcomes: [String: FetchOutcome] = [:]
    /// What one peer published that has not been taken, as last looked at (`openReview(of:)`): to be
    /// taken as shown, or left.
    public private(set) var review: PendingUpdate?
    /// For what both sides did differently: which side to keep, whole, once chosen. By the
    /// name the review gives the conflict: an entry, or a folder (`work/`) for its readers.
    public private(set) var choices: [String: Side] = [:]
    /// The entries not encrypted to exactly the readers their folder names.
    public private(set) var stale: [String] = []

    /// The last failure, for an alert. Set to `nil` to dismiss.
    public var errorMessage: String?

    private let store: SettingsStore
    private let storeDirectory: URL
    private let pasteboard: SecretPasteboard
    private let now: @Sendable () -> Date

    private var service: PassService?
    private var identity: Identity?
    private var lockAt: Date?

    public init(
        store: SettingsStore, storeDirectory: URL, pasteboard: SecretPasteboard,
        now: @escaping @Sendable () -> Date = { Date() }
    ) {
        self.store = store
        self.storeDirectory = storeDirectory
        self.pasteboard = pasteboard
        self.now = now
        settings = store.load()
        loadKeys()
        openStore()
    }

    // MARK: Setup

    public var stage: SetupStage {
        if keyInfo == nil { return .needsKey }
        if dropZone == nil || service == nil { return .needsDropZone }
        return syncState?.hasStore == true ? .ready : .needsStore
    }

    /// The drop zone with this device's SSH key, when everything it needs is there.
    private var dropZone: DropZone? {
        settings.dropZone(sshPrivateKey: store.secret(.sshPrivateKey))
    }

    /// Whether Face ID can unlock: the passphrase was put in the keychain for that.
    public var canUnlockWithBiometrics: Bool {
        keyInfo != nil && settings.passphrasePolicy == .keychainWithBiometrics
    }

    /// What is kept behind Face ID. Marked, so that the empty passphrase of an unprotected
    /// key is still something to keep: Face ID then stands between the phone and the key.
    private static let keptPassphraseMark = "passphrase:"

    private func fail(_ error: Error) {
        errorMessage = message(for: error)
    }

    private func loadKeys() {
        let secretKey = store.secret(.openPGPSecretKey)
        keyInfo = secretKey.flatMap { try? inspectKey(key: $0) }
        ownPublicKey = secretKey.flatMap { try? publicKeyOf(secretKey: $0) }
        deviceKey = store.secret(.sshPrivateKey).flatMap { try? devicePublicKey(privateKey: $0) }
    }

    /// (Re)opens the store with the current settings and keys. `nil` while the settings do not
    /// make a valid set of devices (a bad name, one key under two names...): `configurationProblem` says why.
    private func openStore() {
        service = nil
        configurationProblem = nil
        syncState = nil
        review = nil
        choices = [:]
        outcomes = [:]
        guard let ownPublicKey else { return }
        do {
            let opened = try PassService(config: settings.storeConfig(directory: storeDirectory, ownPublicKey: ownPublicKey))
            service = opened
            syncState = try opened.currentState()
        } catch {
            configurationProblem = message(for: error)
        }
    }

    /// Why the devices as they are set up cannot be used (a name used twice, a key that is not
    /// the one with the fingerprint...). `nil` when they can.
    public private(set) var configurationProblem: String?

    /// Keeps a key that was just checked: the key in the keychain (still protected by its
    /// passphrase), the passphrase too when the policy says so.
    private func keep(secretKey: String, passphrase: String, policy: PassphrasePolicy) async throws {
        let unlocked = try await unlockIdentity(secretKey: secretKey, passphrase: passphrase)
        guard unlocked.canSign() else {
            throw PassError.CannotSign
        }
        try store.setSecret(secretKey, for: .openPGPSecretKey)
        let kept = policy == .keychainWithBiometrics ? Self.keptPassphraseMark + passphrase : nil
        try store.setSecret(kept, for: .passphrase)
        settings.passphrasePolicy = policy
        try store.save(settings)
        loadKeys()
        openStore()
        didUnlock(unlocked)
        await refresh()
    }

    /// Makes this device's OpenPGP key, here: its private half never exists anywhere else.
    /// The public half (`ownPublicKey`) is then for the other devices, which name this one by it.
    @discardableResult
    public func makeKey(passphrase: String, policy: PassphrasePolicy) async -> Bool {
        do {
            // The device's name, so that a `.gpg-id` line written by hand on a computer finds it.
            let name = settings.deviceName
            let made = try await offMain { try generateOpenpgpKey(userId: name, passphrase: passphrase) }
            try await keep(secretKey: made.secretKey, passphrase: passphrase, policy: policy)
            return true
        } catch {
            fail(error)
            return false
        }
    }

    /// Imports a key made elsewhere (`gpg --export-secret-keys --armor <id>`), after checking it and its passphrase.
    @discardableResult
    public func importSecretKey(_ armored: String, passphrase: String, policy: PassphrasePolicy) async -> Bool {
        do {
            let info = try inspectKey(key: armored)
            guard info.isSecret else {
                errorMessage = "This is a public key. Export the secret one: gpg --export-secret-keys --armor <id>"
                return false
            }
            try await keep(secretKey: armored, passphrase: passphrase, policy: policy)
            return true
        } catch {
            fail(error)
            return false
        }
    }

    /// Saves the settings. The store is opened again with them.
    public func save(_ newSettings: PassSettings) {
        do {
            try store.save(newSettings)
            if newSettings.passphrasePolicy == .askEachTime {
                try store.setSecret(nil, for: .passphrase)
            }
            settings = newSettings
            openStore()
        } catch {
            fail(error)
        }
    }

    /// Adds another device, from its armored public key. Its fingerprint is taken from the key:
    /// it is to be compared by eye with what that device shows of itself before relying on it.
    @discardableResult
    public func addPeer(name: String, publicKey: String) -> Bool {
        do {
            guard PassSettings.isDeviceName(name) else {
                errorMessage = "A device name is lowercase letters, digits and hyphens, 32 at most."
                return false
            }
            guard !settings.deviceNames.contains(name) else {
                errorMessage = "There is already a device named \(name)."
                return false
            }
            let info = try inspectKey(key: publicKey)
            guard !info.isSecret else {
                errorMessage = "This is a secret key: a device is added by its public key."
                return false
            }
            guard info.fingerprint != keyInfo?.fingerprint,
                  !settings.peers.contains(where: { $0.fingerprint == info.fingerprint }) else {
                errorMessage = "This key is already a device's."
                return false
            }
            var changed = settings
            changed.peers.append(PeerSettings(name: name, fingerprint: info.fingerprint, publicKey: publicKey))
            save(changed)
            return true
        } catch {
            fail(error)
            return false
        }
    }

    public func removePeer(named name: String) {
        var changed = settings
        changed.peers.removeAll { $0.name == name }
        save(changed)
    }

    /// Makes this device's SSH key, once: the private half goes to the keychain and stays
    /// there, the public half is what the drop zone is given (`DROPZONE_KEY_<NAME>`).
    public func makeDeviceKey() {
        guard deviceKey == nil else { return }
        do {
            let key = try generateDeviceKey(comment: "pass on \(settings.deviceName)")
            try store.setSecret(key.privateKey, for: .sshPrivateKey)
            deviceKey = DeviceKey(privateKey: "", publicKey: key.publicKey, fingerprint: key.fingerprint)
        } catch {
            fail(error)
        }
    }

    /// Removes the store from this device, and everything the app knows: settings and keys.
    /// Nothing is removed in the drop zone; changes that were not published are lost, and a
    /// key made here is gone for good.
    public func eraseEverything() {
        lock()
        try? FileManager.default.removeItem(at: storeDirectory)
        do {
            try store.erase()
        } catch {
            fail(error)
        }
        settings = PassSettings()
        entries = []
        locked = []
        stale = []
        syncMessage = nil
        loadKeys()
        openStore()
    }

    // MARK: Lock

    private func didUnlock(_ unlocked: Identity) {
        identity = unlocked
        isUnlocked = true
        lockAt = now().addingTimeInterval(settings.lockAfter)
    }

    /// Unlocks with a typed passphrase.
    @discardableResult
    public func unlock(passphrase: String) async -> Bool {
        guard let secretKey = store.secret(.openPGPSecretKey) else {
            errorMessage = "No key on this device."
            return false
        }
        do {
            didUnlock(try await unlockIdentity(secretKey: secretKey, passphrase: passphrase))
            return true
        } catch {
            fail(error)
            return false
        }
    }

    /// Unlocks with the passphrase kept in the keychain: reading it is what shows Face ID.
    /// Returns false, with no error, when the user did not authenticate.
    @discardableResult
    public func unlockWithBiometrics() async -> Bool {
        guard canUnlockWithBiometrics, let kept = store.secret(.passphrase),
              kept.hasPrefix(Self.keptPassphraseMark) else { return false }
        return await unlock(passphrase: String(kept.dropFirst(Self.keptPassphraseMark.count)))
    }

    /// Drops the unlocked key. Views holding a decrypted entry watch `isUnlocked` and let go of it.
    public func lock() {
        identity = nil
        isUnlocked = false
        lockAt = nil
    }

    /// The user did something: the lock timeout starts again.
    public func userDidInteract() {
        if isUnlocked {
            lockAt = now().addingTimeInterval(settings.lockAfter)
        }
    }

    /// To call regularly, and when the app comes back: locks once the timeout has passed.
    public func lockIfExpired() {
        if let lockAt, now() >= lockAt {
            lock()
        }
    }

    /// The app is no longer in front: lock at once, nothing stays readable behind the app switcher.
    public func didEnterBackground() {
        lock()
    }

    // MARK: Entries

    /// The entries as folders, when nothing is searched.
    public var tree: [StoreNode] { storeTree(entries, locked: locked) }

    /// The entries whose name holds every word of `searchText`, whatever the case. `nil` when nothing is searched.
    public var searchResults: [String]? {
        let words = searchText.lowercased().split(whereSeparator: \.isWhitespace)
        guard !words.isEmpty else { return nil }
        return entries.filter { name in
            let name = name.lowercased()
            return words.allSatisfy { name.contains($0) }
        }
    }

    /// Reads the list of entries, what is locked or stale, and the sync state again. Needs no
    /// key; with the key, the review that is open is read again too.
    public func refresh() async {
        guard let service else {
            entries = []
            locked = []
            stale = []
            return
        }
        do {
            entries = try await service.entries()
            locked = Set(try await service.unreadable())
            stale = try await service.stale()
            syncState = try await service.syncState()
            if let device = review?.device, let identity {
                setReview(try await service.pendingUpdate(from: device, identity: identity))
            }
        } catch {
            fail(error)
        }
    }

    private func setReview(_ update: PendingUpdate?) {
        if update != review {
            // Another update than the one the choices were made for.
            choices = [:]
            review = update
        }
    }

    /// Who can read an entry: what its folder says, what the file is really encrypted to, and whether this device can.
    public func access(toEntry name: String) async -> Access? {
        try? await service?.access(toEntry: name)
    }

    /// Who can read what is put in a folder (`""` for the top of the store).
    public func access(toFolder folder: String) async -> Access? {
        try? await service?.access(toFolder: folder)
    }

    /// The devices a folder names as its readers itself; `nil` when it names none and has those
    /// of the folder above. For the readers editor.
    public func ownReaders(ofFolder folder: String) async -> [String]? {
        guard let readers = try? await service?.ownReaders(ofFolder: folder) else { return nil }
        return readers.compactMap(\.device)
    }

    /// Names the readers of a folder among `settings.deviceNames`; none gives it those of the
    /// folder above again. What this device reads there is encrypted again for them; what it
    /// does not read stays `stale` until a device that reads it takes the change: naming
    /// oneself is asking, not getting.
    @discardableResult
    public func setReaders(ofFolder folder: String, to readers: [String]) async -> Bool {
        let done: Void? = await withIdentity {
            try await $0.setReaders(ofFolder: folder, to: readers.sorted(), identity: $1)
        }
        if done != nil { await changed() }
        return done != nil
    }

    /// Makes the store here, read by these devices, when no other device has one to take.
    @discardableResult
    public func createStore(readers: [String]) async -> Bool {
        let done: Void? = await withIdentity { try await $0.create(readers: readers.sorted(), identity: $1) }
        if done != nil { await changed() }
        return done != nil
    }

    /// Runs something that needs the unlocked key, and reports its failure.
    private func withIdentity<T: Sendable>(
        _ work: (PassService, Identity) async throws -> T
    ) async -> T? {
        lockIfExpired()
        guard let service, let identity else {
            errorMessage = "The store is locked."
            return nil
        }
        userDidInteract()
        do {
            return try await work(service, identity)
        } catch {
            fail(error)
            return nil
        }
    }

    /// After a change made here: publishes it, as the protocol asks. Without the network it
    /// stays to be published (`syncState.unpublished`), and the next sync sends it: no alert for that.
    private func changed() async {
        await refresh()
        await sync(quietly: true)
    }

    /// Decrypts an entry. The result is the caller's to drop.
    public func show(_ name: String) async -> Entry? {
        await withIdentity { try await $0.show(name, identity: $1) }
    }

    /// The whole text of an entry, to edit it.
    public func content(of name: String) async -> String? {
        await withIdentity { try await $0.content(of: name, identity: $1) }
    }

    /// Adds an entry: the password on the first line of `content`, anything after.
    @discardableResult
    public func add(_ name: String, content: String) async -> Bool {
        let done: Void? = await withIdentity { try await $0.insert(name, content: content, identity: $1) }
        if done != nil { await changed() }
        return done != nil
    }

    /// Adds an entry with a random password, and returns the password.
    public func generate(_ name: String, length: Int, symbols: Bool) async -> String? {
        let password = await withIdentity {
            try await $0.generate(name, length: UInt32(clamping: length), symbols: symbols, identity: $1)
        }
        if password != nil { await changed() }
        return password
    }

    /// Replaces the password of an entry with a random one, the rest kept. Returns the password.
    public func regenerate(_ name: String, length: Int, symbols: Bool) async -> String? {
        let password = await withIdentity {
            try await $0.regenerate(name, length: UInt32(clamping: length), symbols: symbols, identity: $1)
        }
        if password != nil { await changed() }
        return password
    }

    /// Replaces the content of an entry.
    @discardableResult
    public func save(_ name: String, content: String) async -> Bool {
        let done = await withIdentity { try await $0.edit(name, content: content, identity: $1) }
        if done == true { await changed() }
        return done != nil
    }

    @discardableResult
    public func delete(_ name: String) async -> Bool {
        let done: Void? = await withIdentity { try await $0.remove(name, identity: $1) }
        if done != nil { await changed() }
        return done != nil
    }

    @discardableResult
    public func rename(_ name: String, to newName: String) async -> Bool {
        let done: Void? = await withIdentity { try await $0.rename(name, to: newName, identity: $1) }
        if done != nil { await changed() }
        return done != nil
    }

    @discardableResult
    public func deleteFolder(_ folder: String) async -> Bool {
        let done: Void? = await withIdentity { try await $0.removeFolder(folder, identity: $1) }
        if done != nil { await changed() }
        return done != nil
    }

    @discardableResult
    public func renameFolder(_ folder: String, to newName: String) async -> Bool {
        let done: Void? = await withIdentity { try await $0.renameFolder(folder, to: newName, identity: $1) }
        if done != nil { await changed() }
        return done != nil
    }

    /// Puts a secret on the clipboard: for this device only, and for `clipboardExpiry` seconds.
    public func copy(_ text: String) {
        userDidInteract()
        pasteboard.copy(text, expiresAt: now().addingTimeInterval(settings.clipboardExpiry))
    }

    /// The current code of an `otpauth://` line of an entry.
    public func oneTimeCode(for uri: String) -> OtpCode? {
        try? otpCode(uri: uri, unixTime: UInt64(max(0, now().timeIntervalSince1970)))
    }

    // MARK: Sync

    /// The other devices, as they stand: what was received from each, and how much of it is to take.
    public var peers: [PeerState] { syncState?.peers ?? [] }

    /// How long ago a peer signed the last file received from it: "published 3 days ago".
    public func publishedAgo(_ peer: PeerState) -> String? {
        guard peer.received, peer.publishedAt > 0 else { return nil }
        let seconds = max(0, Int(now().timeIntervalSince1970) - Int(peer.publishedAt))
        switch seconds {
        case ..<120: return "published just now"
        case ..<7200: return "published \(seconds / 60) minutes ago"
        case ..<172_800: return "published \(seconds / 3600) hours ago"
        default: return "published \(seconds / 86_400) days ago"
        }
    }

    /// Where a peer stands, in a few words: `2 to take`, or why its file was refused.
    public func standing(of peer: PeerState) -> String {
        switch outcomes[peer.device] {
        case .refused(let reason): return "refused: \(reason)"
        case .stale: return "an older file was served: ignored"
        default: break
        }
        if !peer.received { return "nothing received" }
        return peer.toTake == 0 ? "nothing to take" : "\(peer.toTake) to take"
    }

    /// One visit to the drop zone: publishes this device's tip when it changed, and receives what
    /// every peer published. Nothing received is applied: each peer then has so much "to take",
    /// to be looked at (`openReview(of:)`) and taken or left.
    /// Needs the key: the files are encrypted to it and signed with it.
    public func sync() async {
        await sync(quietly: false)
    }

    private func sync(quietly: Bool) async {
        guard let service, let dropZone else {
            if !quietly { errorMessage = "The sync is not set up yet." }
            return
        }
        guard let identity else {
            if !quietly { errorMessage = "Unlock to sync: the files are signed and encrypted with this device's key." }
            return
        }
        guard !isSyncing else { return }
        isSyncing = true
        defer { isSyncing = false }
        do {
            let report = try await service.sync(dropZone, identity: identity)
            outcomes = Dictionary(uniqueKeysWithValues: report.peers.map { ($0.state.device, $0.outcome) })
            syncMessage = describe(report)
        } catch {
            if quietly {
                syncMessage = "Not published yet: \(message(for: error))"
            } else {
                fail(error)
            }
        }
        await refresh()
    }

    /// Looks at what a peer published that has not been taken: it becomes `review`. `nil` when
    /// there is nothing to take from it. Needs the key.
    @discardableResult
    public func openReview(of device: String) async -> PendingUpdate? {
        let update = await withIdentity { try await $0.pendingUpdate(from: device, identity: $1) }
        guard let update else {
            review = nil
            choices = [:]
            return nil
        }
        setReview(update)
        return review
    }

    /// Leaves the review: nothing is taken, and it is still there to take later.
    public func closeReview() {
        review = nil
        choices = [:]
    }

    /// The review, in the words of SYNC.md, one line per thing it does: `added bank`, `moved a -> b`,
    /// `readers work/: pc phone`, `stale work/w1`, `conflict bank`.
    public var reviewLines: [String] {
        (review?.lines ?? []).map { reviewLineText(line: $0) }
    }

    /// What the review needs a choice for and has none yet.
    public var undecided: [String] {
        review.map { conflicts(in: $0).filter { choices[$0] == nil } } ?? []
    }

    /// Keeps this device's side or the peer's, whole, for one conflict of the review.
    public func choose(_ side: Side, for name: String) {
        choices[name] = side
    }

    /// What a name of the review is on each side, decrypted: where both started, here, and in the update.
    public func versions(of name: String) async -> EntryVersions? {
        guard let device = review?.device else { return nil }
        return await withIdentity { try await $0.versions(of: name, from: device, identity: $1) }
    }

    /// Takes what `review` shows: exactly that. A merge needs every choice made. What taking it
    /// asks this device to encrypt again is encrypted again, and the result is published.
    @discardableResult
    public func take() async -> Bool {
        guard let update = review else { return false }
        guard undecided.isEmpty else {
            errorMessage = "Choose a side for: \(undecided.joined(separator: ", "))."
            return false
        }
        let kept = choices.sorted { $0.key < $1.key }.map { Choice(name: $0.key, keep: $0.value) }
        let taken = await withIdentity { try await $0.take(update, choices: kept, identity: $1) }
        guard let taken else {
            await refresh()
            return false
        }
        review = nil
        choices = [:]
        await changed()
        if !taken.reencrypted.isEmpty {
            syncMessage = "Encrypted \(taken.reencrypted.count) entries again for their readers. " + (syncMessage ?? "")
        }
        return true
    }

    /// Gives up what this device has that a peer's tip does not, and stands where that peer
    /// does. Then publishes where it now stands.
    public func drop(to device: String) async {
        guard let service else { return }
        do {
            let dropped = try await service.drop(to: device)
            await changed()
            syncMessage = "Dropped \(dropped) change(s) of this device."
        } catch {
            fail(error)
        }
    }
}
