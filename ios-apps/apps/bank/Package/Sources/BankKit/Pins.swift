import Foundation

// How the pins get onto the phone.
//
// `bank.py` runs a command (tls-pin-get) that prints the SHA-256 of the certificates to accept. A phone
// runs no command, so the pins are settings, and there are two ways to fill them:
//
// 1. Read it here. The app connects to the host, sends nothing, and shows the SHA-256 of the certificate
//    it is served, with whether it verifies against the public CAs for that name (`PinOffer`). A
//    certificate that does not verify cannot be pinned at all: that is someone in the middle, or a Wi-Fi
//    login page. One that does is as trustworthy as any https site is in a browser, which is not what
//    pinning is for: the screen asks to compare it with what `tls-pin-get` prints on a machine that is
//    already trusted, ideally over another network, and only then is it pinned.
// 2. Paste them. What `tls-pin-get` prints, one SHA-256 per line, sent to the phone by any means that is
//    trusted. Nothing is connected to. Parsed by the core (`parsePins`), as strictly as the CLI does.
//
// Either way a pin is only ever added by the user. When a certificate rotates, requests fail with
// `BankError.PinMismatch`: the app says so (`AppModel.pinProblem`) and sends nothing until the new one is
// pinned the same way. Pins are a list, so the next certificate can be added before the current one goes.

/// The two hosts that are pinned, each with its own pins, as in the CLI's `certificates`.
public enum PinTarget: String, CaseIterable, Identifiable, Sendable {
    /// `thirdparty.qonto.com`: where the credentials go.
    case api
    /// The storage that attachments are downloaded from, by pre-signed URLs. Never gets the credentials.
    case attachments

    public var id: String { rawValue }

    public init(_ host: PinnedHost) {
        switch host {
        case .api: self = .api
        case .attachments: self = .attachments
        }
    }

    public var title: String {
        switch self {
        case .api: return "Qonto API"
        case .attachments: return "Attachments storage"
        }
    }
}

/// As many as the core accepts: the old and the new certificate, a few more, never many.
public let maxPins = 16

public enum PinError: LocalizedError, Equatable {
    case tooMany
    case notAHost(String)
    case notVerifiable

    public var errorDescription: String? {
        switch self {
        case .tooMany:
            return "At most \(maxPins) pins: remove the ones of certificates that are not served anymore."
        case .notAHost(let host):
            return "Not a host name: \(host)"
        case .notVerifiable:
            return "This certificate does not verify against the public CAs for this name: it cannot be pinned."
        }
    }
}

extension BankSettings {
    public func pins(for target: PinTarget) -> [String] {
        switch target {
        case .api: return apiPins
        case .attachments: return attachmentsPins
        }
    }

    private mutating func setPins(_ pins: [String], for target: PinTarget) {
        switch target {
        case .api: apiPins = pins
        case .attachments: attachmentsPins = pins
        }
    }

    /// Adds pins (already normalized by the core) after the ones that are there, without duplicates.
    public mutating func addPins(_ added: [String], for target: PinTarget) throws {
        var pins = pins(for: target)
        for pin in added where !pins.contains(pin) {
            pins.append(pin)
        }
        guard pins.count <= maxPins else { throw PinError.tooMany }
        setPins(pins, for: target)
    }

    public mutating func removePin(_ pin: String, for target: PinTarget) {
        setPins(pins(for: target).filter { $0 != pin }, for: target)
    }

    /// The host whose certificate is read for a target.
    public func host(for target: PinTarget) -> String {
        switch target {
        case .api: return String(apiBaseUrl().dropFirst("https://".count))
        case .attachments: return attachmentsHost.trimmingCharacters(in: .whitespaces).lowercased()
        }
    }

    /// Where the certificate of a target is read. The storage host is typed by the user: it must be
    /// a host name and nothing else.
    public func certificateURL(for target: PinTarget) throws -> String {
        let host = host(for: target)
        let allowed = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyz0123456789.-")
        let labels = host.split(separator: ".", omittingEmptySubsequences: false)
        let valid = host.count <= 253 && labels.count >= 2
            && host.unicodeScalars.allSatisfy(allowed.contains)
            && labels.allSatisfy { !$0.isEmpty && !$0.hasPrefix("-") && !$0.hasSuffix("-") }
        guard valid else { throw PinError.notAHost(host) }
        return "https://\(host)"
    }
}

/// A certificate that was read from a host, waiting to be compared and pinned.
public struct PinOffer: Equatable, Sendable {
    public let target: PinTarget
    public let host: String
    public let sha256: String
    /// It verifies against the public CAs for this host. One that does not is never pinned.
    public let chainValid: Bool
    public let alreadyPinned: Bool

    public var canBePinned: Bool { chainValid && !alreadyPinned }
}

/// A request was refused because the served certificate is none of the pinned ones.
public struct PinProblem: Equatable, Sendable {
    public let target: PinTarget
    public let host: String
    /// SHA-256 of what was served. Not to be pinned from here: read it again, and compare.
    public let served: String
}

/// `ab12cd34…` as `ab12 cd34 …`: easier to compare by eye with what `tls-pin-get` prints.
public func groupedFingerprint(_ sha256: String) -> String {
    var groups: [Substring] = []
    var rest = Substring(sha256)
    while !rest.isEmpty {
        groups.append(rest.prefix(4))
        rest = rest.dropFirst(4)
    }
    return groups.joined(separator: " ")
}
