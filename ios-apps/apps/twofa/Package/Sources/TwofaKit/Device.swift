import Foundation

/// Where a copied code goes.
@MainActor
public protocol Clipboard {
    /// Puts `text` on the clipboard, to be gone from it after `expiresAfter` seconds.
    func copy(_ text: String, expiresAfter: TimeInterval)
}

/// Asks whoever holds the phone to prove they are its owner.
public protocol Authenticator: Sendable {
    func authenticate(reason: String) async -> Bool
}

#if canImport(UIKit)
import UIKit
import UniformTypeIdentifiers

/// The system clipboard, for this device only: a code is not sent to the owner's other
/// devices (Universal Clipboard), and the system forgets it by itself.
public struct SystemClipboard: Clipboard {
    public init() {}

    public func copy(_ text: String, expiresAfter: TimeInterval) {
        UIPasteboard.general.setItems(
            [[UTType.utf8PlainText.identifier: text]],
            options: [.localOnly: true, .expirationDate: Date().addingTimeInterval(expiresAfter)])
    }
}
#endif

#if canImport(LocalAuthentication)
import LocalAuthentication

/// Face ID or Touch ID, and the passcode of the phone when they fail or are not set up.
public struct DeviceAuthenticator: Authenticator {
    public init() {}

    public func authenticate(reason: String) async -> Bool {
        // A context of its own each time: a used one remembers its success.
        (try? await LAContext().evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)) ?? false
    }
}
#endif
