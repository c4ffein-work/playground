#if canImport(Security)
import Foundation
import Security

/// Generic passwords in the keychain of this app.
public struct KeychainSecretStore: SecretStore {
    public struct KeychainError: LocalizedError {
        public let status: OSStatus
        public var errorDescription: String? {
            let text = SecCopyErrorMessageString(status, nil) as String? ?? "error \(status)"
            return "Keychain: \(text)"
        }
    }

    private let service: String

    public init(service: String) {
        self.service = service
    }

    private func query(_ account: String) -> [CFString: Any] {
        [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecAttrAccount: account,
        ]
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
        // The key of a bank account: only while the phone is unlocked (nothing here runs in the background),
        // never in a backup, never restored on another device.
        item[kSecAttrAccessible] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw KeychainError(status: status)
        }
    }
}

extension SettingsStore {
    /// The store of the app.
    public static var shared: SettingsStore {
        SettingsStore(defaults: .standard, secrets: KeychainSecretStore(service: "bank"))
    }
}
#endif
