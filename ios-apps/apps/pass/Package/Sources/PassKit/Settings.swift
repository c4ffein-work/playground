import Foundation
@_exported import PassCore

/// What happens to the passphrase of the OpenPGP key.
public enum PassphrasePolicy: String, Codable, Sendable, CaseIterable {
    /// Typed at each unlock: it is nowhere on the phone.
    case askEachTime
    /// Kept in the keychain behind Face ID: unlocking is a glance.
    case keychainWithBiometrics
}

/// Another device of the sync, as this one was told about it (see SYNC.md, "Devices").
public struct PeerSettings: Codable, Equatable, Sendable, Identifiable {
    public var name: String
    /// 40 hex digits: what was compared by eye with the other device's own screen.
    public var fingerprint: String
    /// Its OpenPGP public key, armored.
    public var publicKey: String

    public var id: String { name }

    public init(name: String, fingerprint: String, publicKey: String) {
        self.name = name
        self.fingerprint = fingerprint
        self.publicKey = publicKey
    }

    var peer: Peer {
        Peer(name: name, fingerprint: fingerprint, publicKey: publicKey)
    }
}

/// Everything about the sync that is not a secret. The secrets are in the keychain: see `SecretAccount`.
public struct PassSettings: Codable, Equatable, Sendable {
    /// This device's name: lowercase letters, digits and hyphens. It names its file in the drop zone.
    public var deviceName = "phone"
    /// `sftp://drop@host:2222`.
    public var dropZoneURL = ""
    /// `SHA256:...` of the drop zone's host key. Nothing is sent to a server that presents another.
    public var dropZoneHostKey: String?
    /// The other devices: equals, each with its own file in the drop zone. Nothing received can
    /// add one or change a key.
    public var peers: [PeerSettings] = []

    public var passphrasePolicy: PassphrasePolicy = .askEachTime
    /// Seconds without use before the store locks itself. Leaving the app locks it at once.
    public var lockAfter: TimeInterval = 120
    /// Seconds a copied password stays on the clipboard.
    public var clipboardExpiry: TimeInterval = 45

    public init() {}

    private var trimmedURL: String { dropZoneURL.trimmingCharacters(in: .whitespacesAndNewlines) }

    /// Every device of the sync by name, this one included, sorted: what a folder's readers are chosen among.
    public var deviceNames: [String] { ([deviceName] + peers.map(\.name)).sorted() }

    /// A usable device name, as the protocol defines it: `[a-z0-9][a-z0-9-]{0,31}`.
    public static func isDeviceName(_ name: String) -> Bool {
        let allowed = Set("abcdefghijklmnopqrstuvwxyz0123456789-")
        return (1...32).contains(name.count) && name.allSatisfy(allowed.contains) && !name.hasPrefix("-")
    }

    /// Enough is filled in to visit the drop zone: this device's name, where the zone is, and its
    /// pinned key. The other devices can come later: a device alone publishes for those to come.
    public var isSyncComplete: Bool {
        Self.isDeviceName(deviceName) && !trimmedURL.isEmpty && dropZoneHostKey != nil
    }

    /// The drop zone, with this device's SSH key. `nil` while something is missing.
    public func dropZone(sshPrivateKey: String?) -> DropZone? {
        guard isSyncComplete, let sshPrivateKey, let dropZoneHostKey else { return nil }
        return DropZone(url: trimmedURL, privateKey: sshPrivateKey, hostKey: dropZoneHostKey)
    }

    /// The core's view of the store in `directory`. `ownPublicKey` is this device's OpenPGP key, armored.
    public func storeConfig(directory: URL, ownPublicKey: String) -> StoreConfig {
        StoreConfig(directory: directory.path, device: deviceName, publicKey: ownPublicKey, peers: peers.map(\.peer))
    }
}

/// The secrets of the app, by the name they are kept under.
public enum SecretAccount: String, Sendable, CaseIterable {
    /// The armored OpenPGP secret key: made here or imported, still protected by its passphrase.
    case openPGPSecretKey = "openpgp-secret-key"
    /// Only with `PassphrasePolicy.keychainWithBiometrics`.
    case passphrase = "openpgp-passphrase"
    /// This device's SSH key for the drop zone, made here, never exported.
    case sshPrivateKey = "ssh-private-key"

    /// Reading it asks for Face ID (or Touch ID), each time.
    public var requiresBiometry: Bool { self == .passphrase }
}

/// Keeps one secret per account name.
public protocol SecretStore: Sendable {
    /// `nil` when there is none, or when the user did not authenticate.
    func read(_ account: String) -> String?
    /// `nil` removes it.
    func write(_ secret: String?, for account: String) throws
}

public final class InMemorySecretStore: SecretStore, @unchecked Sendable {
    private let lock = NSLock()
    private var secrets: [String: String] = [:]

    public init() {}

    public func read(_ account: String) -> String? {
        lock.lock()
        defer { lock.unlock() }
        return secrets[account]
    }

    public func write(_ secret: String?, for account: String) throws {
        lock.lock()
        defer { lock.unlock() }
        secrets[account] = secret
    }
}

/// The settings in `UserDefaults`, the secrets in a `SecretStore`.
public struct SettingsStore: Sendable {
    private static let settingsKey = "pass-settings"

    // UserDefaults is documented as thread-safe.
    private nonisolated(unsafe) let defaults: UserDefaults
    private let secrets: SecretStore

    public init(defaults: UserDefaults, secrets: SecretStore) {
        self.defaults = defaults
        self.secrets = secrets
    }

    public func load() -> PassSettings {
        guard let data = defaults.data(forKey: Self.settingsKey),
              let decoded = try? JSONDecoder().decode(PassSettings.self, from: data) else {
            return PassSettings()
        }
        return decoded
    }

    public func save(_ settings: PassSettings) throws {
        defaults.set(try JSONEncoder().encode(settings), forKey: Self.settingsKey)
    }

    public func secret(_ account: SecretAccount) -> String? {
        secrets.read(account.rawValue)
    }

    /// `nil` or an empty string removes it.
    public func setSecret(_ secret: String?, for account: SecretAccount) throws {
        try secrets.write(secret.flatMap { $0.isEmpty ? nil : $0 }, for: account.rawValue)
    }

    /// Forgets everything: the settings and every secret.
    public func erase() throws {
        defaults.removeObject(forKey: Self.settingsKey)
        for account in SecretAccount.allCases {
            try secrets.write(nil, for: account.rawValue)
        }
    }
}
