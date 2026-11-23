import Foundation
@_exported import TwofaCore

/// Keeps one secret per account name.
///
/// Stricter than a place to put a password, because what is kept here cannot be asked for again:
/// a failed read must never look like "nothing stored" (the next write would erase every key),
/// and a write replaces the secret in one step (a counter half-written is a counter lost).
public protocol SecretStore: Sendable {
    /// `nil` only when there is none. Throws when it cannot tell.
    func read(_ account: String) throws -> String?
    /// Replaces it in one step: afterwards it is the new one, or on failure still the old one. `nil` removes it.
    func write(_ secret: String?, for account: String) throws
}

public final class InMemorySecretStore: SecretStore, @unchecked Sendable {
    private let lock = NSLock()
    private var secrets: [String: String] = [:]

    public init() {}

    public func read(_ account: String) throws -> String? {
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

/// The keys, as the text of a 2fa keychain file (`~/.2fa`), kept whole as one secret.
///
/// One secret rather than one per key: the file is the format the core works on and the one
/// that is imported and exported, so there is nothing to convert and nothing to keep in step
/// (no list of names beside the secrets, no key half-added); and a change of any kind, a
/// counter going up included, is a single replacement of a single item, which is as atomic
/// as the keychain gets. The price is that every change rewrites every key: a few hundred bytes.
public final class KeyStore: @unchecked Sendable {
    private static let account = "keychain-file"

    private let secrets: SecretStore
    /// Serializes read-modify-write: the app has one process, so this is the only lock needed.
    private let lock = NSLock()

    public init(secrets: SecretStore) {
        self.secrets = secrets
    }

    /// The keychain file: empty when there is no key yet.
    public func text() throws -> String {
        lock.lock()
        defer { lock.unlock() }
        return try secrets.read(Self.account) ?? ""
    }

    /// Reads the file, lets `change` make the new one out of it, and stores that.
    /// Returns what `change` returned beside the text, and only once it is stored:
    /// when anything throws, the file is as it was and nothing came out.
    @discardableResult
    public func update<T>(_ change: (String) throws -> (text: String, result: T)) throws -> T {
        lock.lock()
        defer { lock.unlock() }
        let current = try secrets.read(Self.account) ?? ""
        let (text, result) = try change(current)
        if text != current {
            try secrets.write(text.isEmpty ? nil : text, for: Self.account)
        }
        return result
    }
}
