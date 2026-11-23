import PassKit
import UIKit
import UniformTypeIdentifiers

/// The general pasteboard, for secrets: the copy stays on this device (it is not offered
/// to the user's other devices through Universal Clipboard) and the system removes it at the expiry.
struct DevicePasteboard: SecretPasteboard {
    func copy(_ text: String, expiresAt: Date) {
        UIPasteboard.general.setItems(
            [[UTType.utf8PlainText.identifier: text]],
            options: [.localOnly: true, .expirationDate: expiresAt])
    }
}
