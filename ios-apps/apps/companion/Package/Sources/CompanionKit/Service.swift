import Foundation

/// Runs blocking work (every call into the Rust core is) off the calling actor.
func offMain<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
    try await withCheckedThrowingContinuation { continuation in
        DispatchQueue.global(qos: .userInitiated).async {
            continuation.resume(with: Result { try work() })
        }
    }
}

/// Turns the core's byte counts into a 0...1 fraction, told at most once per percent.
final class ProgressRelay: ProgressListener, @unchecked Sendable {
    private let handler: @Sendable (Double) -> Void
    private let lock = NSLock()
    private var lastPercent: UInt64 = .max

    init(_ handler: @escaping @Sendable (Double) -> Void) {
        self.handler = handler
    }

    func onProgress(done: UInt64, total: UInt64) {
        guard total > 0 else { return }
        let percent = min(done, total) * 100 / total
        lock.lock()
        let changed = percent != lastPercent
        lastPercent = percent
        lock.unlock()
        if changed {
            handler(Double(percent) / 100)
        }
    }
}

/// What to show for an error: the core's own sentence for its errors, the system's for the others.
public func message(for error: Error) -> String {
    if let error = error as? CompanionError {
        return errorMessage(error: error)
    }
    return error.localizedDescription
}

/// Reads the certificate of an https server, to show its fingerprint before it is pinned.
public func fetchCertificate(url: String) async throws -> CertificateFingerprint {
    try await offMain { try fetchFingerprint(url: url, extraCaPem: nil) }
}

/// `ab12cd34…` as `ab12 cd34 …`: easier to compare by eye with `companion fingerprint` on the server.
public func groupedFingerprint(_ sha256: String) -> String {
    var groups: [Substring] = []
    var rest = Substring(sha256)
    while !rest.isEmpty {
        groups.append(rest.prefix(4))
        rest = rest.dropFirst(4)
    }
    return groups.joined(separator: " ")
}

/// The companion server, as `async` calls.
public final class CompanionService: Sendable {
    private let client: CompanionClient

    /// Validates the settings. Connects to nothing.
    public init(settings: ServerSettings, secret: String) throws {
        client = try CompanionClient(config: settings.config(secret: secret))
    }

    /// Newest first.
    public func files() async throws -> [RemoteFile] {
        let client = client
        return try await offMain { Array(try client.listFiles().reversed()) }
    }

    /// Streams the file from disk: its size is not a memory problem (a share extension has ~120 MB).
    public func upload(
        fileAt url: URL,
        named name: String? = nil,
        progress: (@Sendable (Double) -> Void)? = nil
    ) async throws -> UploadedFile {
        let client = client
        return try await offMain {
            try client.uploadFile(path: url.path, filename: name, progress: progress.map(ProgressRelay.init))
        }
    }

    public func upload(data: Data, named name: String) async throws -> UploadedFile {
        let client = client
        return try await offMain { try client.uploadBytes(filename: name, content: data) }
    }

    /// Where `download` puts a file: `directory/<id>/<name>`, so two files of the same name do not collide.
    public static func localURL(for file: RemoteFile, in directory: URL) -> URL {
        var name = file.name.replacingOccurrences(of: "/", with: "_")
        if name.isEmpty || name.hasPrefix(".") {
            name = "file" + name
        }
        return directory.appendingPathComponent(file.id).appendingPathComponent(name)
    }

    /// Downloads into `directory` and returns the local file. A file never changes
    /// on the server under the same id, so one already there in full is not fetched again.
    public func download(
        _ file: RemoteFile,
        into directory: URL,
        progress: (@Sendable (Double) -> Void)? = nil
    ) async throws -> URL {
        let client = client
        let local = Self.localURL(for: file, in: directory)
        return try await offMain {
            let existing = try? FileManager.default.attributesOfItem(atPath: local.path)[.size] as? NSNumber
            if existing?.uint64Value == file.size {
                return local
            }
            try FileManager.default.createDirectory(
                at: local.deletingLastPathComponent(), withIntermediateDirectories: true)
            _ = try client.downloadFile(
                fileId: file.id, destPath: local.path, progress: progress.map(ProgressRelay.init))
            return local
        }
    }

    public func pad() async throws -> Pad {
        let client = client
        return try await offMain { try client.getPad() }
    }

    /// Replaces the pad: last write wins. Returns the new pad timestamp.
    @discardableResult
    public func setPad(_ content: String) async throws -> UInt64 {
        let client = client
        return try await offMain { try client.setPad(content: content) }
    }

    /// Adds text at the end of the pad, on its own line.
    @discardableResult
    public func appendToPad(_ text: String) async throws -> UInt64 {
        let client = client
        return try await offMain {
            let current = try client.getPad().content
            let separator = current.isEmpty || current.hasSuffix("\n") ? "" : "\n"
            return try client.setPad(content: current + separator + text + "\n")
        }
    }

    /// What every client is asked to show.
    public func preview() async throws -> PreviewState {
        let client = client
        return try await offMain { try client.previewCurrent() }
    }

    /// Asks every client to show this file.
    @discardableResult
    public func present(fileId: String) async throws -> UInt64 {
        let client = client
        return try await offMain { try client.setPreview(fileId: fileId) }
    }
}

extension RemoteFile: Identifiable {}

extension RemoteFile {
    /// The server sends its local time with no zone: read as the phone's, good enough on one LAN.
    public var uploadedDate: Date? {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd'T'HH:mm:ss"
        return formatter.date(from: String(uploaded.prefix(19)))
    }

    public var sizeText: String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: size), countStyle: .file)
    }

    /// An SF Symbol for the kind of file.
    public var symbolName: String {
        switch mimetype.split(separator: "/").first.map(String.init) ?? "" {
        case "image": return "photo"
        case "video": return "film"
        case "audio": return "waveform"
        case "text": return "doc.text"
        default:
            if mimetype == "application/pdf" { return "doc.richtext" }
            if mimetype.contains("zip") || mimetype.contains("tar") || mimetype.contains("compressed") {
                return "doc.zipper"
            }
            return "doc"
        }
    }
}
