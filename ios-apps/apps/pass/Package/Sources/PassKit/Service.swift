import Foundation

/// Runs blocking work (every call into the Rust core is) off the calling actor.
func offMain<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
    try await withCheckedThrowingContinuation { continuation in
        DispatchQueue.global(qos: .userInitiated).async {
            continuation.resume(with: Result { try work() })
        }
    }
}

/// What to show for an error: the core's own sentence for its errors, the system's for the others.
public func message(for error: Error) -> String {
    if let error = error as? PassError {
        return errorMessage(error: error)
    }
    return error.localizedDescription
}

/// A fingerprint in groups of four, easier to compare by eye: `ABCD 1234 ...`.
public func groupedFingerprint(_ fingerprint: String) -> String {
    var groups: [Substring] = []
    var rest = Substring(fingerprint)
    while !rest.isEmpty {
        groups.append(rest.prefix(4))
        rest = rest.dropFirst(4)
    }
    return groups.joined(separator: " ")
}

/// A folder or an entry of the store, for the list.
public struct StoreNode: Identifiable, Equatable, Sendable {
    /// The last part of the path: `work`.
    public let name: String
    /// The whole path: `Email/work`. For an entry, its name in the store.
    public let path: String
    public let isFolder: Bool
    /// An entry this device cannot decrypt: listed, by name only.
    public let isLocked: Bool
    /// Folders first, then entries, each sorted without regard to case. Empty for an entry.
    public let children: [StoreNode]

    /// A folder and an entry may have the same path (`a/` and `a.gpg`).
    public var id: String { isFolder ? path + "/" : path }

    /// How many entries are below.
    public var entryCount: Int {
        isFolder ? children.reduce(0) { $0 + $1.entryCount } : 1
    }
}

/// The entries as a tree: `["Email/work", "bank"]` becomes a folder `Email` holding `work`, and `bank`.
/// `locked` are the entries this device cannot read.
public func storeTree(_ names: [String], locked: Set<String> = []) -> [StoreNode] {
    func build(_ names: [(path: String, rest: Substring)], prefix: String) -> [StoreNode] {
        var folders: [String: [(path: String, rest: Substring)]] = [:]
        var entries: [StoreNode] = []
        for (path, rest) in names {
            if let slash = rest.firstIndex(of: "/") {
                folders[String(rest[..<slash]), default: []].append((path, rest[rest.index(after: slash)...]))
            } else {
                entries.append(StoreNode(
                    name: String(rest), path: path, isFolder: false, isLocked: locked.contains(path), children: []))
            }
        }
        let byName: (StoreNode, StoreNode) -> Bool = {
            let order = $0.name.localizedCaseInsensitiveCompare($1.name)
            return order == .orderedSame ? $0.name < $1.name : order == .orderedAscending
        }
        let folderNodes = folders.map { name, below in
            StoreNode(
                name: name, path: prefix + name, isFolder: true, isLocked: false,
                children: build(below, prefix: prefix + name + "/"))
        }
        return folderNodes.sorted(by: byName) + entries.sorted(by: byName)
    }
    return build(names.map { ($0, Substring($0)) }, prefix: "")
}

/// One line saying where the device stands, for under the sync button.
public func describe(_ state: SyncState) -> String {
    guard state.hasStore else {
        return state.updateWaiting ? "The owner's store has arrived: look at it to take it." : "No store yet: sync to receive the owner's."
    }
    var parts: [String] = []
    if state.updateWaiting { parts.append("an update from the owner is waiting") }
    if state.outstanding > 0 { parts.append("\(state.outstanding) change(s) of this device not taken by the owner yet") }
    if state.unpublished { parts.append("not published yet") }
    guard let first = parts.first else { return "In step with the owner." }
    return first.prefix(1).uppercased() + first.dropFirst() + parts.dropFirst().map { ", " + $0 }.joined() + "."
}

/// Who can read something, in a few words: `pc, phone`, with what is no device's key told as such.
public func describe(_ readers: [Reader]) -> String {
    readers.map { $0.device ?? "unknown key \($0.id.suffix(8))" }.joined(separator: ", ")
}

/// What is wrong with who can read an entry, if anything: for a warning under "Readable by".
public func warnings(for access: Access) -> [String] {
    var warnings: [String] = []
    if access.readers.contains(where: { $0.device == nil }) {
        warnings.append("One of its readers is a key this device does not know: it cannot write here.")
    }
    if !access.matches {
        warnings.append(
            "The file is encrypted to \(access.encryptedTo.joined(separator: ", ")), not to the readers its folder lists: it was not encrypted again after they changed.")
    }
    return warnings
}

/// What an update does to an entry, in a word.
public func describe(_ kind: ChangeKind) -> String {
    switch kind {
    case .added: return "added"
    case .changed: return "changed"
    case .removed: return "removed"
    case .renamed(let from): return "renamed from \(from)"
    }
}

/// The store, as `async` calls. Holds no secret: the identity is passed to what needs it.
public final class PassService: Sendable {
    private let store: PassStore

    /// Opens the store, which may not be there yet. Reads nothing.
    public init(config: StoreConfig) throws {
        store = try PassStore.open(config: config)
    }

    public func entries() async throws -> [String] {
        let store = store
        return try await offMain { try store.list() }
    }

    /// The entries this device cannot decrypt.
    public func unreadable() async throws -> [String] {
        let store = store
        return try await offMain { try store.unreadable() }
    }

    public func access(toEntry name: String) async throws -> Access {
        let store = store
        return try await offMain { try store.entryAccess(name: name) }
    }

    public func access(toFolder folder: String) async throws -> Access {
        let store = store
        return try await offMain { try store.folderAccess(folder: folder) }
    }

    public func show(_ name: String, identity: Identity) async throws -> Entry {
        let store = store
        return try await offMain { try store.show(name: name, identity: identity) }
    }

    /// The whole decrypted text of an entry, to edit it.
    public func content(of name: String, identity: Identity) async throws -> String {
        let store = store
        return try await offMain { try store.content(name: name, identity: identity) }
    }

    public func insert(_ name: String, content: String, identity: Identity) async throws {
        let store = store
        try await offMain { try store.insert(name: name, content: content, overwrite: false, identity: identity) }
    }

    /// Returns false when the content is what the entry already holds.
    @discardableResult
    public func edit(_ name: String, content: String, identity: Identity) async throws -> Bool {
        let store = store
        return try await offMain { try store.edit(name: name, content: content, identity: identity) }
    }

    /// A new entry with a random password, which is returned.
    public func generate(_ name: String, length: UInt32, symbols: Bool, identity: Identity) async throws -> String {
        let store = store
        return try await offMain {
            try store.generate(name: name, length: length, symbols: symbols, overwrite: false, identity: identity)
        }
    }

    /// A new random password for an existing entry, the rest of it kept. Returns the password.
    public func regenerate(_ name: String, length: UInt32, symbols: Bool, identity: Identity) async throws -> String {
        let store = store
        return try await offMain {
            try store.regenerate(name: name, length: length, symbols: symbols, identity: identity)
        }
    }

    public func remove(_ name: String, identity: Identity) async throws {
        let store = store
        try await offMain { try store.remove(name: name, identity: identity) }
    }

    public func rename(_ name: String, to newName: String, identity: Identity) async throws {
        let store = store
        try await offMain { try store.rename(from: name, to: newName, overwrite: false, identity: identity) }
    }

    /// The same, at once: it reads a few references of the local repository, nothing more.
    public func currentState() throws -> SyncState {
        try store.syncState()
    }

    /// Where the device stands, from what it has: no network.
    public func syncState() async throws -> SyncState {
        let store = store
        return try await offMain { try store.syncState() }
    }

    /// One visit to the drop zone: publishes this device's tip when it changed, and receives the owner's file.
    public func sync(_ zone: DropZone, identity: Identity) async throws -> SyncReport {
        let store = store
        return try await offMain { try store.sync(zone: zone, identity: identity) }
    }

    /// What the owner published that has not been taken, described.
    public func pendingUpdate() async throws -> PendingUpdate? {
        let store = store
        return try await offMain { try store.pendingUpdate() }
    }

    /// An entry of the pending update, decrypted: before, here, and in the update.
    public func versions(of name: String, identity: Identity) async throws -> EntryVersions {
        let store = store
        return try await offMain { try store.updateVersions(name: name, identity: identity) }
    }

    /// Takes the update that was shown, and no other. Returns the entries that changed.
    public func accept(_ update: PendingUpdate, choices: [Choice], identity: Identity) async throws -> [String] {
        let store = store
        return try await offMain {
            try store.acceptUpdate(commit: update.commit, choices: choices, identity: identity)
        }
    }

    /// Gives up this device's commits the owner has not taken. Returns how many were dropped.
    public func discardOutstanding() async throws -> UInt32 {
        let store = store
        return try await offMain { try store.discardOutstanding() }
    }
}

/// Unlocks a secret key off the main thread: deriving the key from the passphrase is slow on purpose.
public func unlockIdentity(secretKey: String, passphrase: String) async throws -> Identity {
    try await offMain { try Identity.unlock(secretKey: secretKey, passphrase: passphrase) }
}

/// Reads the host key of a drop zone, to show its fingerprint before it is pinned.
public func readHostKey(url: String) async throws -> HostKey {
    try await offMain { try fetchHostKey(url: url) }
}
