import Foundation

/// A feed the user follows. Its cookies are not here: they go to the keychain.
public struct Feed: Codable, Equatable, Identifiable, Sendable {
    public let id: UUID
    public var name: String
    /// `http://…` or `https://…`, as the core checked it.
    public var url: String

    public init(id: UUID = UUID(), name: String, url: String) {
        self.id = id
        self.name = name
        self.url = url
    }
}

/// Cookies given for a plain http feed cross the network readable by anyone on the way:
/// the form says so before they are saved.
public func cookiesWouldBeSentInClear(url: String, cookies: String) -> Bool {
    let cookies = cookies.trimmingCharacters(in: .whitespacesAndNewlines)
    return !cookies.isEmpty && url.trimmingCharacters(in: .whitespaces).lowercased().hasPrefix("http://")
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

    public var isEmpty: Bool {
        lock.lock()
        defer { lock.unlock() }
        return secrets.isEmpty
    }
}

/// The list of feeds and the settings in `UserDefaults`, the cookies of each feed in a `SecretStore`.
public struct FeedStore: Sendable {
    private static let feedsKey = "feeds"
    private static let imagesKey = "load-images"

    // UserDefaults is documented as thread-safe.
    private nonisolated(unsafe) let defaults: UserDefaults
    private let secrets: SecretStore

    public init(defaults: UserDefaults, secrets: SecretStore) {
        self.defaults = defaults
        self.secrets = secrets
    }

    public func loadFeeds() -> [Feed] {
        guard let data = defaults.data(forKey: Self.feedsKey) else { return [] }
        return (try? JSONDecoder().decode([Feed].self, from: data)) ?? []
    }

    public func save(_ feeds: [Feed]) throws {
        defaults.set(try JSONEncoder().encode(feeds), forKey: Self.feedsKey)
    }

    private static func account(_ feed: UUID) -> String {
        "cookies-\(feed.uuidString)"
    }

    /// The value of the `Cookie` header to send for this feed, if it needs one.
    public func cookies(for feed: UUID) -> String? {
        secrets.read(Self.account(feed))
    }

    /// Empty or `nil` removes them.
    public func setCookies(_ cookies: String?, for feed: UUID) throws {
        let trimmed = cookies?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        try secrets.write(trimmed.isEmpty ? nil : trimmed, for: Self.account(feed))
    }

    /// Whether an article may load its images from the network. Off until the user says otherwise.
    public var loadImages: Bool {
        defaults.bool(forKey: Self.imagesKey)
    }

    public func setLoadImages(_ load: Bool) {
        defaults.set(load, forKey: Self.imagesKey)
    }
}
