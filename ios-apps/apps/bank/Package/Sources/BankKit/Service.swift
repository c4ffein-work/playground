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
    if let error = error as? BankError {
        return errorMessage(error: error)
    }
    return error.localizedDescription
}

/// The Qonto API, as `async` calls.
public final class BankService: Sendable {
    private let client: BankClient

    /// The real API. Validates the settings; connects to nothing.
    public init(settings: BankSettings, credentials: Credentials) throws {
        client = try BankClient(config: settings.config(credentials))
    }

    /// For tests: a mock of the API on this machine, over plain http (so, with no pin).
    /// The core refuses anything but a loopback address.
    init(mockAt baseURL: String, credentials: Credentials) throws {
        client = try BankClient.withBaseUrl(
            config: BankSettings().config(credentials), baseUrl: baseURL, extraCaPem: nil)
    }

    public func accounts() async throws -> [BankAccount] {
        let client = client
        return try await offMain { try client.accounts() }
    }

    /// One page of at most 100 transactions, newest first. Pages start at 1.
    public func transactions(
        accountId: String, filter: TransactionFilter, page: UInt32
    ) async throws -> TransactionsPage {
        let client = client
        return try await offMain {
            try client.transactionsPage(bankAccountId: accountId, filter: filter, page: page)
        }
    }

    /// With fresh download URLs: they do not last.
    public func attachments(transactionId: String) async throws -> [Attachment] {
        let client = client
        return try await offMain { try client.attachments(transactionId: transactionId) }
    }

    /// Downloads into `directory` and returns the local file, named by the core after the id of the
    /// attachment and what its content is. One that is already there is not fetched again.
    public func download(
        _ attachment: Attachment,
        into directory: URL,
        progress: (@Sendable (Double) -> Void)? = nil
    ) async throws -> LocalAttachment {
        let client = client
        return try await offMain {
            try client.downloadAttachment(
                attachment: attachment, directory: directory.path, progress: progress.map(ProgressRelay.init))
        }
    }

    /// Sends a PDF, a JPEG or a PNG to a transaction. Unless `force`, a file that the transaction
    /// already has is not sent again: what it has is downloaded into `directory` to be compared.
    public func attach(
        fileAt url: URL,
        to transactionId: String,
        force: Bool,
        attachmentsDirectory directory: URL,
        progress: (@Sendable (Double) -> Void)? = nil
    ) async throws -> AttachOutcome {
        let client = client
        return try await offMain {
            try client.attachFile(
                transactionId: transactionId, path: url.path, force: force,
                attachmentsDirectory: directory.path, progress: progress.map(ProgressRelay.init))
        }
    }
}

/// What the app model reaches the outside world through: the real one, or what a test puts there.
public struct BankEnvironment: Sendable {
    var makeService: @Sendable (BankSettings, Credentials) throws -> BankService
    /// Reads the certificate served at an https URL, sending nothing.
    var readFingerprint: @Sendable (String) throws -> CertificateFingerprint

    public static let production = BankEnvironment(
        makeService: { try BankService(settings: $0, credentials: $1) },
        readFingerprint: { try fetchFingerprint(url: $0, extraCaPem: nil) })
}

// MARK: Receipts

/// What a file is, from its first bytes: `nil` for what Qonto does not accept (anything but PDF, JPEG, PNG).
public func receiptType(of data: Data) -> FileType? {
    detectFileType(head: data.prefix(16))
}

public func receiptType(ofFileAt url: URL) -> FileType? {
    guard let handle = try? FileHandle(forReadingFrom: url) else { return nil }
    defer { try? handle.close() }
    return receiptType(of: (try? handle.read(upToCount: 16)) ?? Data())
}

/// Writes a picked image where it can be uploaded from, and returns that file: a temporary one, to remove
/// after the upload. A JPEG, a PNG or a PDF is kept as it is, byte for byte. Anything else (the photo
/// library gives HEIC) goes through `convert`, which is expected to make a JPEG of it: UIKit does that,
/// so the app provides it.
public func prepareReceipt(
    _ data: Data,
    in directory: URL = FileManager.default.temporaryDirectory,
    convert: (Data) -> Data?
) throws -> URL {
    var content = data
    if receiptType(of: content) == nil, let converted = convert(data) {
        content = converted
    }
    guard let type = receiptType(of: content) else { throw BankError.UnsupportedFile }
    let folder = directory.appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    let url = folder.appendingPathComponent("receipt.\(type.extension)")
    try content.write(to: url, options: .atomic)
    return url
}
