import Foundation
import Observation

/// Where copied secrets go. On the phone: the general pasteboard, for this device
/// only (no Universal Clipboard) and until the expiry (see `App/Pasteboard.swift`).
public protocol SecretPasteboard: Sendable {
    func copy(_ text: String, expiresAt: Date)
}

/// What is missing before the store can be used, in the order the setup asks for it.
public enum SetupStage: Equatable, Sendable {
    /// No OpenPGP secret key has been imported.
    case needsKey
    /// The relay is not fully described (URL, pinned host key or certificate, device key or token).
    case needsRemote
    /// Everything is there but the store itself: clone it.
    case needsClone
    case ready
}

/// The main branch has moved and this device has changes that are not in it.
public struct Divergence: Equatable, Sendable {
    public let unmerged: UInt32
    public let behind: UInt32
}

/// Everything the views show, and everything they can ask for. No view talks to the core itself.
///
/// The unlocked key lives here and nowhere else: `lock()` drops it, and with it the
/// only thing that can read an entry. Nothing decrypted is kept by the model.
@MainActor
@Observable
public final class AppModel {
    public private(set) var settings: PassSettings
    /// The imported OpenPGP key, described. `nil` when there is none.
    public private(set) var keyInfo: KeyInfo?
    /// The public half of this device's SSH key, for the relay's `authorized_keys`.
    public private(set) var deviceKey: DeviceKey?
    public private(set) var hasHTTPSToken = false

    /// Every entry of the store, sorted.
    public private(set) var entries: [String] = []
    /// The `.gpg-id` files, with the recipients this device has no public key for.
    public private(set) var recipientFiles: [RecipientFile] = []
    /// What the search field holds. Each word narrows the list.
    public var searchText = ""

    public private(set) var isUnlocked = false
    public private(set) var syncState: SyncState?
    public private(set) var isSyncing = false
    /// What the last sync did, in a sentence.
    public private(set) var syncMessage: String?
    /// An update that changes who the store is encrypted to: to be shown, then accepted or left.
    public private(set) var pendingUpdate: PendingUpdate?
    public private(set) var divergence: Divergence?

    /// The last failure, for an alert. Set to `nil` to dismiss.
    public var errorMessage: String?

    private let store: SettingsStore
    private let storeDirectory: URL
    private let pasteboard: SecretPasteboard
    private let now: @Sendable () -> Date

    private var service: PassService?
    private var identity: Identity?
    private var ownPublicKey: String?
    private var lockAt: Date?
    private var hasStore = false

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
        if remote == nil { return .needsRemote }
        return hasStore ? .ready : .needsClone
    }

    /// The relay with its secrets, when everything it needs is there.
    private var remote: Remote? {
        settings.remote(
            sshPrivateKey: store.secret(.sshPrivateKey), httpsToken: store.secret(.httpsToken))
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
        hasHTTPSToken = store.secret(.httpsToken) != nil
    }

    /// (Re)opens the store with the current settings and keys. Reads nothing of it.
    private func openStore() {
        hasStore = FileManager.default.fileExists(atPath: storeDirectory.appendingPathComponent(".git").path)
        service = try? PassService(config: settings.storeConfig(directory: storeDirectory, ownPublicKey: ownPublicKey))
    }

    /// Checks the key and its passphrase, then keeps the key in the keychain (as exported:
    /// still protected by the passphrase), and the passphrase too when the policy says so.
    @discardableResult
    public func importSecretKey(_ armored: String, passphrase: String, policy: PassphrasePolicy) async -> Bool {
        do {
            let info = try inspectKey(key: armored)
            guard info.isSecret else {
                errorMessage = "This is a public key. Export the secret one: gpg --export-secret-keys --armor <id>"
                return false
            }
            let unlocked = try await unlockIdentity(secretKey: armored, passphrase: passphrase)
            try store.setSecret(armored, for: .openPGPSecretKey)
            let kept = policy == .keychainWithBiometrics ? Self.keptPassphraseMark + passphrase : nil
            try store.setSecret(kept, for: .passphrase)
            settings.passphrasePolicy = policy
            try store.save(settings)
            loadKeys()
            openStore()
            didUnlock(unlocked)
            await refresh()
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

    public func setHTTPSToken(_ token: String) {
        do {
            try store.setSecret(token, for: .httpsToken)
            hasHTTPSToken = !token.isEmpty
        } catch {
            fail(error)
        }
    }

    /// Makes this device's SSH key, once: the private half goes to the keychain and stays
    /// there, the public half is to be added to the relay's `authorized_keys`.
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

    /// The line for the relay's `authorized_keys` that lets this device push to its own
    /// branch and nowhere else (with the `update` hook of `apps/pass/relay`).
    public var authorizedKeysLine: String? {
        guard let deviceKey else { return nil }
        let device = settings.deviceName
        return #"command="PASS_DEVICE=\#(device) exec git-shell -c \"$SSH_ORIGINAL_COMMAND\"",restrict \#(deviceKey.publicKey)"#
    }

    /// Clones the relay. The store is then there, to be looked at (`recipientFiles`) before it is written to.
    @discardableResult
    public func cloneStore() async -> Bool {
        guard let remote else {
            errorMessage = "The relay is not set up yet."
            return false
        }
        isSyncing = true
        defer { isSyncing = false }
        do {
            let config = settings.storeConfig(directory: storeDirectory, ownPublicKey: ownPublicKey)
            service = try await PassService.clone(config: config, remote: remote)
            hasStore = true
            syncMessage = "Cloned."
            await refresh()
            return true
        } catch {
            fail(error)
            return false
        }
    }

    /// Removes the store from this device, and everything the app knows: settings, keys, token.
    /// Nothing is removed on the relay; changes that were not pushed are lost.
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
        recipientFiles = []
        syncState = nil
        syncMessage = nil
        pendingUpdate = nil
        divergence = nil
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
            errorMessage = "No secret key on this device."
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
    public var tree: [StoreNode] { storeTree(entries) }

    /// The entries whose name holds every word of `searchText`, whatever the case. `nil` when nothing is searched.
    public var searchResults: [String]? {
        let words = searchText.lowercased().split(whereSeparator: \.isWhitespace)
        guard !words.isEmpty else { return nil }
        return entries.filter { name in
            let name = name.lowercased()
            return words.allSatisfy { name.contains($0) }
        }
    }

    /// Reads the list of entries, the recipients and the sync state again. Needs no key.
    public func refresh() async {
        guard let service, hasStore else {
            entries = []
            return
        }
        do {
            entries = try await service.entries()
            recipientFiles = try await service.recipientFiles()
            syncState = try await service.syncState()
        } catch {
            fail(error)
        }
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
        await refresh()
        return done != nil
    }

    /// Adds an entry with a random password, and returns the password.
    public func generate(_ name: String, length: Int, symbols: Bool) async -> String? {
        let password = await withIdentity {
            try await $0.generate(name, length: UInt32(clamping: length), symbols: symbols, identity: $1)
        }
        await refresh()
        return password
    }

    /// Replaces the password of an entry with a random one, the rest kept. Returns the password.
    public func regenerate(_ name: String, length: Int, symbols: Bool) async -> String? {
        let password = await withIdentity {
            try await $0.regenerate(name, length: UInt32(clamping: length), symbols: symbols, identity: $1)
        }
        await refresh()
        return password
    }

    /// Replaces the content of an entry.
    @discardableResult
    public func save(_ name: String, content: String) async -> Bool {
        let done = await withIdentity { try await $0.edit(name, content: content, identity: $1) }
        await refresh()
        return done != nil
    }

    @discardableResult
    public func delete(_ name: String) async -> Bool {
        let done: Void? = await withIdentity { try await $0.remove(name, identity: $1) }
        await refresh()
        return done != nil
    }

    @discardableResult
    public func rename(_ name: String, to newName: String) async -> Bool {
        let done: Void? = await withIdentity { try await $0.rename(name, to: newName, identity: $1) }
        await refresh()
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

    private func took(_ outcome: PullOutcome) -> String {
        switch outcome {
        case .upToDate:
            pendingUpdate = nil
            divergence = nil
            return "Nothing new."
        case .fastForwarded(let commits, _):
            pendingUpdate = nil
            divergence = nil
            return "Took \(commits) change(s)."
        case .needsReview(let update):
            pendingUpdate = update
            divergence = nil
            return "An update changes who the store is encrypted to: review it."
        case .diverged(let unmerged, let behind):
            divergence = Divergence(unmerged: unmerged, behind: behind)
            return "\(behind) new change(s) cannot be taken until this device's \(unmerged) are merged on the computer."
        }
    }

    /// Takes what the main branch has (fast-forward only, never a merge) and sends this
    /// device's changes to its own branch. Needs no key: the store can sync while locked.
    public func sync() async {
        guard let service, let remote, hasStore else {
            errorMessage = "The relay is not set up yet."
            return
        }
        guard !isSyncing else { return }
        isSyncing = true
        defer { isSyncing = false }
        do {
            let report = try await service.sync(remote)
            var parts = [took(report.pull)]
            if report.pushed > 0 {
                parts.append("Sent \(report.pushed) change(s) to \(report.state.deviceBranch).")
            }
            syncMessage = parts.joined(separator: " ")
        } catch {
            fail(error)
        }
        await refresh()
    }

    /// Takes the update that `pendingUpdate` shows: exactly that one, with its new recipients.
    public func acceptPendingUpdate() async {
        guard let service, let update = pendingUpdate else { return }
        do {
            syncMessage = took(try await service.accept(update))
        } catch {
            fail(error)
        }
        await refresh()
    }

    /// Gives up this device's unmerged changes and goes back to the main branch.
    public func discardLocalChanges() async {
        guard let service, let remote else { return }
        isSyncing = true
        defer { isSyncing = false }
        do {
            let dropped = try await service.discardLocalCommits(remote)
            divergence = nil
            syncMessage = "Dropped \(dropped) change(s) of this device."
        } catch {
            fail(error)
        }
        await refresh()
    }
}
