#if canImport(Security)
import Foundation
import Security

/// Generic passwords in the keychain: the same store as CompanionKit's.
public struct KeychainSecretStore: SecretStore {
    public struct KeychainError: LocalizedError {
        public let status: OSStatus
        public var errorDescription: String? {
            let text = SecCopyErrorMessageString(status, nil) as String? ?? "error \(status)"
            return "Keychain: \(text)"
        }
    }

    private let service: String
    private let accessGroup: String?

    public init(service: String, accessGroup: String? = nil) {
        self.service = service
        self.accessGroup = accessGroup
    }

    private func query(_ account: String) -> [CFString: Any] {
        var query: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
        ]
        if let accessGroup {
            query[kSecAttrAccessGroup] = accessGroup
        }
        return query
    }

    public func read(_ account: String) -> String? {
        var query = query(account)
        query[kSecReturnData] = true
        query[kSecMatchLimit] = kSecMatchLimitOne
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess, let data = item as? Data else {
            return nil
        }
        return String(data: data, encoding: .utf8)
    }

    public func write(_ secret: String?, for account: String) throws {
        let query = query(account)
        var status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw KeychainError(status: status)
        }
        guard let secret else { return }
        var item = query
        item[kSecValueData] = Data(secret.utf8)
        // Readable while the phone is in use, never restored on another device.
        item[kSecAttrAccessible] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw KeychainError(status: status)
        }
    }
}

extension FeedStore {
    /// The app's own store.
    public static var shared: FeedStore {
        FeedStore(defaults: .standard, secrets: KeychainSecretStore(service: "feed"))
    }
}
#endif
