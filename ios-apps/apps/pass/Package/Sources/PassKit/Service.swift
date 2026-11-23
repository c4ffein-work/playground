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
public func storeTree(_ names: [String]) -> [StoreNode] {
    func build(_ names: [(path: String, rest: Substring)], prefix: String) -> [StoreNode] {
        var folders: [String: [(path: String, rest: Substring)]] = [:]
        var entries: [StoreNode] = []
        for (path, rest) in names {
            if let slash = rest.firstIndex(of: "/") {
                folders[String(rest[..<slash]), default: []].append((path, rest[rest.index(after: slash)...]))
            } else {
                entries.append(StoreNode(name: String(rest), path: path, isFolder: false, children: []))
            }
        }
        let byName: (StoreNode, StoreNode) -> Bool = {
            let order = $0.name.localizedCaseInsensitiveCompare($1.name)
            return order == .orderedSame ? $0.name < $1.name : order == .orderedAscending
        }
        let folderNodes = folders.map { name, below in
            StoreNode(name: name, path: prefix + name, isFolder: true, children: build(below, prefix: prefix + name + "/"))
        }
        return folderNodes.sorted(by: byName) + entries.sorted(by: byName)
    }
    return build(names.map { ($0, Substring($0)) }, prefix: "")
}

/// One line saying where the device stands, for under the sync button.
public func describe(_ state: SyncState) -> String {
    guard state.isRepository else { return "Not synced: the store is not a git repository." }
    var parts: [String] = []
    if state.unmerged > 0 {
        let waiting = state.unmerged - state.unpushed
        if state.unpushed > 0 { parts.append("\(state.unpushed) change(s) to send") }
        if waiting > 0 { parts.append("\(waiting) change(s) sent, waiting to be merged") }
    }
    if state.behind > 0 { parts.append("\(state.behind) new change(s) on \(state.branch)") }
    return parts.isEmpty ? "Up to date with \(state.branch)." : parts.joined(separator: ", ") + "."
}

/// The store, as `async` calls. Holds no secret: the identity is passed to what needs it.
public final class PassService: Sendable {
    private let store: PassStore

    /// Opens the store. Reads nothing yet.
    public init(config: StoreConfig) throws {
        store = try PassStore.open(config: config)
    }

    private init(store: PassStore) {
        self.store = store
    }

    /// Clones the relay into the (empty or absent) directory of `config`.
    public static func clone(config: StoreConfig, remote: Remote) async throws -> PassService {
        try await offMain { PassService(store: try cloneStore(config: config, remote: remote)) }
    }

    public func entries() async throws -> [String] {
        let store = store
        return try await offMain { try store.list() }
    }

    public func recipientFiles() async throws -> [RecipientFile] {
        let store = store
        return try await offMain { try store.recipientFiles() }
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

    /// Where the device stands, from what it has: no network.
    public func syncState() async throws -> SyncState {
        let store = store
        return try await offMain { try store.syncState() }
    }

    /// Takes the main branch (fast-forward only) and sends this device's commits to its own branch.
    public func sync(_ remote: Remote) async throws -> SyncReport {
        let store = store
        return try await offMain { try store.sync(remote: remote) }
    }

    /// Takes the update that was shown, and no other.
    public func accept(_ update: PendingUpdate) async throws -> PullOutcome {
        let store = store
        return try await offMain { try store.acceptUpdate(commit: update.commit) }
    }

    /// Gives up this device's unmerged changes. Returns how many commits were dropped.
    public func discardLocalCommits(_ remote: Remote) async throws -> UInt32 {
        let store = store
        return try await offMain { try store.discardLocalCommits(remote: remote) }
    }
}

/// Unlocks a secret key off the main thread: deriving the key from the passphrase is slow on purpose.
public func unlockIdentity(secretKey: String, passphrase: String) async throws -> Identity {
    try await offMain { try Identity.unlock(secretKey: secretKey, passphrase: passphrase) }
}

/// Reads the host key of an SSH server, to show its fingerprint before it is pinned.
public func readHostKey(url: String) async throws -> HostKey {
    try await offMain { try fetchHostKey(url: url) }
}

/// Reads the certificate of an https server, to show its fingerprint before it is pinned.
public func readCertificate(url: String) async throws -> CertificateFingerprint {
    try await offMain { try fetchFingerprint(url: url, extraCaPem: nil) }
}
