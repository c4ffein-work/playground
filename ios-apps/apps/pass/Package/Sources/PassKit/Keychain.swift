#if canImport(Security) && canImport(LocalAuthentication)
import Foundation
import LocalAuthentication
import Security

/// Generic passwords in the keychain of this device, and of no other: nothing here
/// goes into a backup, to iCloud Keychain or to a new phone.
///
/// The accounts in `biometricAccounts` are also behind Face ID (or Touch ID): reading
/// one shows the system prompt, and enrolling another face or finger makes it unreadable.
public struct KeychainSecretStore: SecretStore {
    public struct KeychainError: LocalizedError {
        public let status: OSStatus
        public var errorDescription: String? {
            let text = SecCopyErrorMessageString(status, nil) as String? ?? "error \(status)"
            return "Keychain: \(text)"
        }
    }

    private let service: String
    private let biometricAccounts: Set<String>
    private let prompt: String

    public init(service: String, biometricAccounts: Set<String>, prompt: String) {
        self.service = service
        self.biometricAccounts = biometricAccounts
        self.prompt = prompt
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
        if biometricAccounts.contains(account) {
            let context = LAContext()
            context.localizedReason = prompt
            query[kSecUseAuthenticationContext] = context
        }
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
        if biometricAccounts.contains(account) {
            var error: Unmanaged<CFError>?
            // `biometryCurrentSet`: no passcode fallback, and void once the enrolled faces or fingers change.
            guard let access = SecAccessControlCreateWithFlags(
                nil, kSecAttrAccessibleWhenPasscodeSetThisDeviceOnly, .biometryCurrentSet, &error) else {
                throw KeychainError(status: errSecParam)
            }
            item[kSecAttrAccessControl] = access
        } else {
            // Only while the phone is unlocked, never restored on another device.
            item[kSecAttrAccessible] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        }
        status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw KeychainError(status: status)
        }
    }
}

extension SettingsStore {
    /// The store of the app itself.
    public static var shared: SettingsStore {
        SettingsStore(
            defaults: .standard,
            secrets: KeychainSecretStore(
                service: "pass",
                biometricAccounts: Set(SecretAccount.allCases.filter(\.requiresBiometry).map(\.rawValue)),
                prompt: "Unlock the password store"))
    }
}
#endif
