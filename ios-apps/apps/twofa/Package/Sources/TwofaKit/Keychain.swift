#if canImport(Security)
import Foundation
import Security

/// Generic passwords in the keychain of this app alone: no access group, no iCloud.
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

    public func read(_ account: String) throws -> String? {
        var query = query(account)
        query[kSecReturnData] = true
        query[kSecMatchLimit] = kSecMatchLimitOne
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecItemNotFound {
            return nil
        }
        // Anything else (the phone locked again, say) is not "no keys": it must not be written over.
        guard status == errSecSuccess, let data = item as? Data, let secret = String(data: data, encoding: .utf8) else {
            throw KeychainError(status: status == errSecSuccess ? errSecDecode : status)
        }
        return secret
    }

    public func write(_ secret: String?, for account: String) throws {
        let query = query(account)
        guard let secret else {
            let status = SecItemDelete(query as CFDictionary)
            guard status == errSecSuccess || status == errSecItemNotFound else {
                throw KeychainError(status: status)
            }
            return
        }
        // Update in place: deleting then adding would leave nothing if the second step failed.
        let data = Data(secret.utf8)
        var status = SecItemUpdate(query as CFDictionary, [kSecValueData: data] as CFDictionary)
        if status == errSecItemNotFound {
            var item = query
            item[kSecValueData] = data
            // Readable only while the phone is unlocked; never synced, never restored on another device.
            item[kSecAttrAccessible] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
            item[kSecAttrSynchronizable] = false
            status = SecItemAdd(item as CFDictionary, nil)
        }
        guard status == errSecSuccess else {
            throw KeychainError(status: status)
        }
    }
}

extension KeyStore {
    /// The keys of this device.
    public static var device: KeyStore {
        KeyStore(secrets: KeychainSecretStore(service: "twofa"))
    }
}
#endif
