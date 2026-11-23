import Foundation
@_exported import PassCore

/// How the relay is reached.
public enum Transport: String, Codable, Sendable, CaseIterable {
    /// A bare repository on a machine of one's own, with a key made on this device.
    case ssh
    /// A git hosting service, or one's own server behind https, with a token.
    case https
}

/// What happens to the passphrase of the OpenPGP key.
public enum PassphrasePolicy: String, Codable, Sendable, CaseIterable {
    /// Typed at each unlock: it is nowhere on the phone.
    case askEachTime
    /// Kept in the keychain behind Face ID: unlocking is a glance.
    case keychainWithBiometrics
}

/// Everything about the store that is not a secret. The secrets are in the keychain: see `SecretAccount`.
public struct PassSettings: Codable, Equatable, Sendable {
    public var transport: Transport = .ssh
    /// `ssh://user@host[:port]/path` or `user@host:path`; `https://host/path`.
    public var url = ""
    /// `SHA256:...` of the SSH server's host key. There is no SSH without it.
    public var sshHostKey: String?
    public var httpsUsername = ""
    /// SHA-256 of the pinned https certificate.
    public var pinSHA256: String?
    /// On top of the pin, verify chain and hostname against the public CAs.
    public var verifyChain = false
    /// No pin: trust the public CAs, as a browser. For a hosting service, whose certificate changes.
    public var trustPublicCAs = false

    /// Names this device's branch on the relay: `devices/<deviceName>`.
    public var deviceName = "phone"
    /// Who this device's commits are by.
    public var authorName = ""
    public var authorEmail = ""

    /// Armored public keys of the other recipients of the store. This device's own comes from its secret key.
    public var otherPublicKeys: [String] = []
    /// As `PASSWORD_STORE_SIGNING_KEY`: when not empty, a `.gpg-id` must be signed by one of these fingerprints.
    public var signingKeys: [String] = []
    /// Only take updates whose last commit is signed by a key of the store.
    public var requireSignedUpdates = false

    public var passphrasePolicy: PassphrasePolicy = .askEachTime
    /// Seconds without use before the store locks itself. Leaving the app locks it at once.
    public var lockAfter: TimeInterval = 120
    /// Seconds a copied password stays on the clipboard.
    public var clipboardExpiry: TimeInterval = 45

    public init() {}

    private var trimmedURL: String { url.trimmingCharacters(in: .whitespacesAndNewlines) }

    public var isHTTPS: Bool { trimmedURL.lowercased().hasPrefix("https://") }

    /// Plain http: nothing protects the connection, and no token is sent over it.
    public var isPlainHTTP: Bool { trimmedURL.lowercased().hasPrefix("http://") }

    /// How the https server is trusted: `nil` over plain http, and while nothing has been chosen.
    public var trust: TlsTrust? {
        guard isHTTPS else { return nil }
        if trustPublicCAs { return .publicCa }
        guard let pin = pinSHA256 else { return nil }
        return verifyChain ? .pinnedWithCa(sha256: pin, extraCaPem: nil) : .pinnedOnly(sha256: pin)
    }

    /// Enough is filled in to try the relay.
    public var isRemoteComplete: Bool {
        guard !trimmedURL.isEmpty, !deviceName.isEmpty else { return false }
        switch transport {
        case .ssh: return sshHostKey != nil
        case .https: return isPlainHTTP || trust != nil
        }
    }

    public var hasAuthor: Bool { !authorName.isEmpty && !authorEmail.isEmpty }

    /// The remote, with its secrets. `nil` while something is missing (for SSH: the device's key).
    public func remote(sshPrivateKey: String?, httpsToken: String?) -> Remote? {
        guard isRemoteComplete else { return nil }
        switch transport {
        case .ssh:
            guard let sshPrivateKey, let sshHostKey else { return nil }
            return .ssh(url: trimmedURL, privateKey: sshPrivateKey, hostKey: sshHostKey)
        case .https:
            let token = isHTTPS ? httpsToken : nil
            return .http(
                url: trimmedURL, username: httpsUsername.isEmpty ? nil : httpsUsername,
                password: token, trust: trust)
        }
    }

    /// The core's view of the store in `directory`. `ownPublicKey` is this device's, armored.
    public func storeConfig(directory: URL, ownPublicKey: String?) -> StoreConfig {
        StoreConfig(
            directory: directory.path,
            publicKeys: (ownPublicKey.map { [$0] } ?? []) + otherPublicKeys,
            signingKeys: signingKeys,
            requireSignedUpdates: requireSignedUpdates,
            authorName: authorName.isEmpty ? "pass on \(deviceName)" : authorName,
            authorEmail: authorEmail.isEmpty ? "\(deviceName)@pass.invalid" : authorEmail,
            device: deviceName)
    }
}

/// The secrets of the app, by the name they are kept under.
public enum SecretAccount: String, Sendable, CaseIterable {
    /// The armored OpenPGP secret key, as exported: still protected by its passphrase.
    case openPGPSecretKey = "openpgp-secret-key"
    /// Only with `PassphrasePolicy.keychainWithBiometrics`.
    case passphrase = "openpgp-passphrase"
    /// This device's SSH key, made here, never exported.
    case sshPrivateKey = "ssh-private-key"
    case httpsToken = "https-token"

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
