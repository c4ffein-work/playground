import Foundation
@_exported import CompanionCore

/// Where the server is and how it is trusted. The client secret is not here: it goes to the keychain.
public struct ServerSettings: Codable, Equatable, Sendable {
    /// `http://host:port` or `https://host[:port]`
    public var url: String
    public var clientId: String
    /// SHA-256 of the pinned certificate, for https. There is no https without it.
    public var pinSHA256: String?
    /// On top of the pin, verify chain and hostname against the public CAs.
    /// Off for a self-signed certificate: the pin alone identifies it.
    public var verifyChain: Bool

    public init(url: String = "", clientId: String = "", pinSHA256: String? = nil, verifyChain: Bool = false) {
        self.url = url
        self.clientId = clientId
        self.pinSHA256 = pinSHA256
        self.verifyChain = verifyChain
    }

    public var isHTTPS: Bool {
        url.trimmingCharacters(in: .whitespaces).lowercased().hasPrefix("https://")
    }

    /// Enough is filled in to try to connect.
    public var isComplete: Bool {
        !url.isEmpty && !clientId.isEmpty && (!isHTTPS || pinSHA256 != nil)
    }

    public var trust: TlsTrust? {
        guard isHTTPS, let pin = pinSHA256 else { return nil }
        return verifyChain ? .pinnedWithCa(sha256: pin, extraCaPem: nil) : .pinnedOnly(sha256: pin)
    }

    public func config(secret: String) -> ServerConfig {
        ServerConfig(url: url, clientId: clientId, clientSecret: secret, trust: trust)
    }
}

/// Keeps one secret per account name.
public protocol SecretStore: Sendable {
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

/// The settings in `UserDefaults`, the client secret in a `SecretStore`.
/// The app and its share extension see the same ones through their app group.
public struct SettingsStore: Sendable {
    /// Also the keychain access group. Must match the entitlements of both targets (project.yml).
    public static let appGroup = "group.dev.cafeine.companion"

    private static let settingsKey = "server-settings"
    private static let secretAccount = "client-secret"

    // UserDefaults is documented as thread-safe.
    private nonisolated(unsafe) let defaults: UserDefaults
    private let secrets: SecretStore

    public init(defaults: UserDefaults, secrets: SecretStore) {
        self.defaults = defaults
        self.secrets = secrets
    }

    public func load() -> (settings: ServerSettings, secret: String) {
        var settings = ServerSettings()
        if let data = defaults.data(forKey: Self.settingsKey),
           let decoded = try? JSONDecoder().decode(ServerSettings.self, from: data) {
            settings = decoded
        }
        return (settings, secrets.read(Self.secretAccount) ?? "")
    }

    public func save(_ settings: ServerSettings, secret: String) throws {
        defaults.set(try JSONEncoder().encode(settings), forKey: Self.settingsKey)
        try secrets.write(secret.isEmpty ? nil : secret, for: Self.secretAccount)
    }
}
