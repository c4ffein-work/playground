import Foundation
import Observation

/// A key as a row of the list. The secret is not in it.
public struct KeyItem: Identifiable, Equatable, Sendable {
    public let name: String
    public let digits: Int
    /// Counter-based (HOTP): a code is made when asked for, not as time passes.
    public let isCounterBased: Bool
    /// Time-based: the code right now. Counter-based: the last one asked for since the app was opened, if any.
    public let code: String?

    public var id: String { name }
}

/// What the "add a key" form edits.
public struct KeyDraft: Equatable, Sendable {
    public var name = ""
    /// Base32, as typed: spaces and case do not matter.
    public var secret = ""
    public var digits = 6
    public var isCounterBased = false
    /// The last counter used, for a counter-based key: 0 for a new one.
    public var counter: UInt64 = 0

    public init() {}

    /// From an `otpauth://` URI: what a QR code says. Throws for what the app cannot give right codes for.
    public init(uri: String) throws {
        let key = try parseOtpauth(uri: uri)
        name = key.suggestedName
        secret = key.secret
        digits = Int(key.digits)
        isCounterBased = key.counter != nil
        counter = key.counter ?? 0
    }
}

/// What to show for an error: the core's own sentence for its errors, the system's for the others.
public func message(for error: Error) -> String {
    if let error = error as? TwofaError {
        return errorMessage(error: error)
    }
    return error.localizedDescription
}

/// `123456` as `123 456`: easier to read and to type. Only for the eye: what is copied has no space.
public func groupedCode(_ code: String) -> String {
    let half = (code.count + 1) / 2
    return code.count < 6 ? code : "\(code.prefix(half)) \(code.dropFirst(half))"
}

extension ImportOutcome {
    /// What an import did, in a few sentences.
    public var summary: String {
        func count(_ n: Int, _ one: String, _ many: String) -> String { n == 1 ? "1 \(one)" : "\(n) \(many)" }
        var sentences = [count(added.count, "key added.", "keys added.")]
        if !advanced.isEmpty {
            sentences.append(count(advanced.count, "counter moved forward.", "counters moved forward."))
        }
        if !unchanged.isEmpty {
            sentences.append(count(unchanged.count, "was already there.", "were already there."))
        }
        if !conflicts.isEmpty {
            sentences.append(
                "Kept as they were, because the file has another key under the same name: "
                    + conflicts.joined(separator: ", ") + ".")
        }
        if !skipped.isEmpty {
            sentences.append(count(skipped.count, "line was not a key.", "lines were not keys."))
        }
        return sentences.joined(separator: " ")
    }
}

/// Everything the views show, and everything they can ask for. No view reads a secret or computes a code.
@MainActor
@Observable
public final class AppModel {
    /// A copied code leaves the clipboard after this long.
    public static let clipboardExpiry: TimeInterval = 60
    /// How long a row says it was copied.
    static let copiedFeedback: TimeInterval = 2
    static let lockOnOpenKey = "lock-on-open"

    /// Sorted by name, as `2fa` lists them. Empty while locked.
    public private(set) var keys: [KeyItem] = []
    /// Lines of the stored file that are not keys: kept, exported, shown nowhere else.
    public private(set) var skippedLines: [SkippedLine] = []
    /// Until the time-based codes change: 30 down to 1.
    public private(set) var secondsRemaining = 30
    /// Nothing is shown and nothing can be asked for until `unlock()`.
    public private(set) var isLocked: Bool
    /// Ask for Face ID (or the passcode) each time the app is opened. Changed with `setLockOnOpen`.
    public private(set) var lockOnOpen: Bool
    /// The name of the key whose code was just copied, for a moment.
    public private(set) var copied: String?

    /// The last failure, for an alert. Set to `nil` to dismiss.
    public var errorMessage: String?

    private let store: KeyStore
    // UserDefaults is documented as thread-safe.
    private let defaults: UserDefaults
    private let clipboard: Clipboard
    private let authenticator: Authenticator
    private let now: () -> Date

    /// The 30 second step the codes in `keys` belong to.
    private var step: UInt64?
    /// Counter-based codes are not in the file: the ones asked for are remembered until the app locks or closes.
    private var counterCodes: [String: String] = [:]
    private var copiedUntil: Date?
    /// One question at a time, however many things ask for the unlock.
    private var isUnlocking = false

    public init(
        store: KeyStore,
        defaults: UserDefaults,
        clipboard: Clipboard,
        authenticator: Authenticator,
        now: @escaping () -> Date = { Date() }
    ) {
        self.store = store
        self.defaults = defaults
        self.clipboard = clipboard
        self.authenticator = authenticator
        self.now = now
        let lockOnOpen = defaults.bool(forKey: Self.lockOnOpenKey)
        self.lockOnOpen = lockOnOpen
        isLocked = lockOnOpen
        refresh()
    }

    private var unixTime: UInt64 {
        UInt64(max(0, now().timeIntervalSince1970))
    }

    private func fail(_ error: Error) {
        errorMessage = message(for: error)
    }

    // MARK: Codes

    /// Reads the keys and computes their codes for the current time.
    private func refresh() {
        guard !isLocked else { return }
        let time = unixTime
        secondsRemaining = Int(TwofaCore.secondsRemaining(unixTime: time))
        do {
            let keychain = readKeychain(text: try store.text(), unixTime: time)
            keys = keychain.keys.map { key in
                KeyItem(
                    name: key.name, digits: Int(key.digits), isCounterBased: key.counter != nil,
                    code: key.code ?? counterCodes[key.name])
            }
            skippedLines = keychain.skipped
            step = time / 30
        } catch {
            fail(error)
        }
    }

    /// To call about once a second while on screen: moves the countdown, and changes the codes when
    /// their time is up. Returns how long until the next second starts, which is when to call it again.
    @discardableResult
    public func tick() -> TimeInterval {
        let date = now()
        if let until = copiedUntil, date >= until {
            (copied, copiedUntil) = (nil, nil)
        }
        if !isLocked {
            let time = unixTime
            if step != time / 30 {
                refresh()
            } else {
                secondsRemaining = Int(TwofaCore.secondsRemaining(unixTime: time))
            }
        }
        let seconds = date.timeIntervalSince1970
        return max(0.05, seconds.rounded(.down) + 1 - seconds)
    }

    /// The next code of a counter-based key. The counter is stored first: if that fails, no code is shown.
    public func nextCode(for name: String) {
        guard !isLocked else { return }
        do {
            counterCodes[name] = try store.update { text in
                let step = try nextHotp(text: text, name: name)
                return (step.text, step.code)
            }
            refresh()
        } catch {
            fail(error)
        }
    }

    /// Copies the code a key shows, for this device only and for a minute.
    public func copy(_ name: String) {
        guard !isLocked, let code = keys.first(where: { $0.name == name })?.code else { return }
        clipboard.copy(code, expiresAfter: Self.clipboardExpiry)
        copied = name
        copiedUntil = now().addingTimeInterval(Self.copiedFeedback)
    }

    // MARK: Keys

    /// Stores what `change` makes of the keychain file, and shows the result. Returns whether it worked.
    private func change(_ change: (String) throws -> String) -> Bool {
        guard !isLocked else { return false }
        do {
            try store.update { (try change($0), ()) }
            refresh()
            return true
        } catch {
            fail(error)
            return false
        }
    }

    /// The draft for what a scanned QR code or a pasted link says, to be reviewed and then added.
    /// `nil`, with the reason in `errorMessage`, when it is not a key this app can hold.
    public func draft(fromURI uri: String) -> KeyDraft? {
        do {
            return try KeyDraft(uri: uri)
        } catch {
            fail(error)
            return nil
        }
    }

    /// Returns whether the key was added; the reason is in `errorMessage` when it was not.
    @discardableResult
    public func add(_ draft: KeyDraft) -> Bool {
        change { text in
            try addKey(
                text: text, name: draft.name.trimmingCharacters(in: .whitespaces),
                digits: UInt8(clamping: draft.digits), secret: draft.secret,
                counter: draft.isCounterBased ? draft.counter : nil)
        }
    }

    /// There is no undo: the secret is gone.
    @discardableResult
    public func remove(_ name: String) -> Bool {
        let removed = change { try removeKey(text: $0, name: name) }
        if removed {
            counterCodes[name] = nil
        }
        return removed
    }

    @discardableResult
    public func rename(_ name: String, to newName: String) -> Bool {
        let newName = newName.trimmingCharacters(in: .whitespaces)
        let renamed = change { try renameKey(text: $0, name: name, newName: newName) }
        if renamed, name != newName, let code = counterCodes.removeValue(forKey: name) {
            counterCodes[newName] = code
            refresh()
        }
        return renamed
    }

    // MARK: Import and export

    /// The keychain file, as `2fa` reads it at `~/.2fa`: every secret, in clear.
    public func exportText() -> String? {
        guard !isLocked else { return nil }
        do {
            return try store.text()
        } catch {
            fail(error)
            return nil
        }
    }

    /// Merges a 2fa keychain file into the keys: see `import_keychain` in the core for what wins.
    /// Returns what happened, or `nil` with the reason in `errorMessage`.
    public func importText(_ incoming: String) -> ImportOutcome? {
        guard !isLocked else { return nil }
        do {
            let outcome = try store.update { text in
                let outcome = importKeychain(text: text, incoming: incoming)
                if outcome.added.isEmpty && outcome.advanced.isEmpty {
                    return (text, outcome) // nothing to take: what is stored is not touched
                }
                return (outcome.text, outcome)
            }
            refresh()
            return outcome
        } catch {
            fail(error)
            return nil
        }
    }

    /// The same, from the bytes of a file.
    public func importFile(_ data: Data) -> ImportOutcome? {
        guard let text = String(data: data, encoding: .utf8) else {
            errorMessage = "This is not a 2fa file: it is not text."
            return nil
        }
        return importText(text)
    }

    // MARK: Lock

    /// Turning it on asks for the proof it will then ask each time: a phone that cannot give it
    /// (no Face ID and no passcode) would lock the keys in. Turning it off asks nothing more
    /// than being in the app already does.
    public func setLockOnOpen(_ enabled: Bool) async {
        if enabled && !lockOnOpen {
            guard await authenticator.authenticate(reason: "Lock 2fa") else {
                errorMessage = "The lock was not turned on: this device could not confirm it is you."
                return
            }
        }
        lockOnOpen = enabled
        defaults.set(enabled, forKey: Self.lockOnOpenKey)
    }

    /// To call when the app leaves the screen. Forgets every code; does nothing when the lock is off.
    public func lock() {
        guard lockOnOpen else { return }
        isLocked = true
        keys = []
        skippedLines = []
        counterCodes = [:]
        step = nil
        (copied, copiedUntil) = (nil, nil)
    }

    /// Asks for Face ID (or the passcode), and shows the keys if it is given.
    public func unlock() async {
        guard isLocked, !isUnlocking else { return }
        isUnlocking = true
        defer { isUnlocking = false }
        if await authenticator.authenticate(reason: "Show your codes") {
            isLocked = false
            refresh()
        }
    }
}
