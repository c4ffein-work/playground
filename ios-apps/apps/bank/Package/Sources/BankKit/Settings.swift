import Foundation
@_exported import BankCore

/// The login of the Qonto API. Never in the settings: both halves go to the keychain.
public struct Credentials: Equatable, Sendable {
    /// The organization slug, as Qonto shows it next to the secret key.
    public var organizationSlug: String
    public var secretKey: String

    public init(organizationSlug: String = "", secretKey: String = "") {
        self.organizationSlug = organizationSlug
        self.secretKey = secretKey
    }

    public var isComplete: Bool { !organizationSlug.isEmpty && !secretKey.isEmpty }
}

/// What is not a secret: which certificates are trusted, and which account is shown.
/// There is no URL here: the API is where it is (`apiBaseUrl()`), and that is not a setting.
public struct BankSettings: Codable, Equatable, Sendable {
    /// SHA-256 of the certificates accepted from the API. There is no connection without one.
    public var apiPins: [String]
    /// The same for the storage attachments are downloaded from. Without one, nothing is downloaded.
    public var attachmentsPins: [String]
    /// The host of that storage: only where its certificate is read from before it is pinned.
    public var attachmentsHost: String
    /// The bank account whose transactions are listed. `nil`: the first one, as the CLI.
    public var selectedAccountId: String?

    public init(
        apiPins: [String] = [],
        attachmentsPins: [String] = [],
        attachmentsHost: String = defaultAttachmentsHost(),
        selectedAccountId: String? = nil
    ) {
        self.apiPins = apiPins
        self.attachmentsPins = attachmentsPins
        self.attachmentsHost = attachmentsHost
        self.selectedAccountId = selectedAccountId
    }

    /// Enough to try to connect, with complete credentials.
    public var isComplete: Bool { !apiPins.isEmpty }

    public func config(_ credentials: Credentials) -> BankConfig {
        BankConfig(
            organizationSlug: credentials.organizationSlug,
            secretKey: credentials.secretKey,
            apiPins: apiPins,
            attachmentsPins: attachmentsPins)
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

/// The settings in `UserDefaults`, the credentials in a `SecretStore`.
public struct SettingsStore: Sendable {
    private static let settingsKey = "bank-settings"
    private static let slugAccount = "organization-slug"
    private static let secretAccount = "secret-key"

    // UserDefaults is documented as thread-safe.
    private nonisolated(unsafe) let defaults: UserDefaults
    private let secrets: SecretStore

    public init(defaults: UserDefaults, secrets: SecretStore) {
        self.defaults = defaults
        self.secrets = secrets
    }

    public func load() -> (settings: BankSettings, credentials: Credentials) {
        var settings = BankSettings()
        if let data = defaults.data(forKey: Self.settingsKey),
           let decoded = try? JSONDecoder().decode(BankSettings.self, from: data) {
            settings = decoded
        }
        let credentials = Credentials(
            organizationSlug: secrets.read(Self.slugAccount) ?? "",
            secretKey: secrets.read(Self.secretAccount) ?? "")
        return (settings, credentials)
    }

    /// The pins and the selected account: nothing of the keychain is touched.
    public func save(_ settings: BankSettings) throws {
        defaults.set(try JSONEncoder().encode(settings), forKey: Self.settingsKey)
    }

    /// Empty credentials are removed from the keychain.
    public func save(_ credentials: Credentials) throws {
        let slug = credentials.organizationSlug
        try secrets.write(slug.isEmpty ? nil : slug, for: Self.slugAccount)
        try secrets.write(credentials.secretKey.isEmpty ? nil : credentials.secretKey, for: Self.secretAccount)
    }
}
