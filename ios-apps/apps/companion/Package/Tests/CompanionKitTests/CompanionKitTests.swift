import Foundation
import XCTest
@testable import CompanionKit

final class SettingsTests: XCTestCase {
    func testTrustFollowsTheURL() {
        var settings = ServerSettings(url: "http://10.0.0.2:8080", clientId: "phone")
        XCTAssertTrue(settings.isComplete)
        XCTAssertNil(settings.trust)

        settings.url = "https://companion.example"
        XCTAssertFalse(settings.isComplete, "https with no pin is not usable")
        XCTAssertThrowsError(try CompanionService(settings: settings, secret: "ab"))

        let pin = String(repeating: "ab", count: 32)
        settings.pinSHA256 = pin
        XCTAssertTrue(settings.isComplete)
        XCTAssertEqual(settings.trust, .pinnedOnly(sha256: pin))
        settings.verifyChain = true
        XCTAssertEqual(settings.trust, .pinnedWithCa(sha256: pin, extraCaPem: nil))
        XCTAssertNoThrow(try CompanionService(settings: settings, secret: "ab"))

        // A pin left over from an https URL is not sent with an http one (the core would refuse it).
        settings.url = "http://10.0.0.2:8080"
        XCTAssertNil(settings.trust)
    }

    func testErrorsFromTheCoreAreReadable() {
        XCTAssertThrowsError(try CompanionService(settings: ServerSettings(url: "http://h:1", clientId: "a b"), secret: "ab")) {
            XCTAssertEqual(message(for: $0), "the client id is 1 to 64 letters, digits and hyphens")
        }
    }

    func testStoreRoundTrip() throws {
        let suite = "companion-tests-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = SettingsStore(defaults: defaults, secrets: InMemorySecretStore())

        XCTAssertEqual(store.load().settings, ServerSettings())
        XCTAssertEqual(store.load().secret, "")

        let settings = ServerSettings(url: "https://h", clientId: "phone", pinSHA256: "ab", verifyChain: true)
        try store.save(settings, secret: "c0ffee")
        XCTAssertEqual(store.load().settings, settings)
        XCTAssertEqual(store.load().secret, "c0ffee")

        try store.save(settings, secret: "")
        XCTAssertEqual(store.load().secret, "")
    }

    func testFingerprintGrouping() {
        XCTAssertEqual(groupedFingerprint("ab12cd34ef"), "ab12 cd34 ef")
        XCTAssertEqual(groupedFingerprint(""), "")
    }

    func testRemoteFilePresentation() {
        let file = RemoteFile(
            id: "id-1", name: "../a/b.pdf", size: 1_500_000, mimetype: "application/pdf",
            uploaded: "2026-11-23T08:30:15.123456", uploadedBy: "laptop")
        XCTAssertEqual(file.symbolName, "doc.richtext")
        let parts = Calendar.current.dateComponents([.year, .month, .day, .hour, .minute, .second], from: file.uploadedDate!)
        XCTAssertEqual([parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second], [2026, 11, 23, 8, 30, 15])
        // The name comes from the server: it must not walk out of the download directory.
        let local = CompanionService.localURL(for: file, in: URL(fileURLWithPath: "/downloads"))
        XCTAssertEqual(local.path, "/downloads/id-1/file.._a_b.pdf")
        var hidden = file
        hidden.name = ".."
        XCTAssertEqual(CompanionService.localURL(for: hidden, in: URL(fileURLWithPath: "/downloads")).path, "/downloads/id-1/file..")
    }
}

/// Collects what a progress handler is told.
final class Fractions: @unchecked Sendable {
    private let lock = NSLock()
    private var values: [Double] = []

    func add(_ value: Double) {
        lock.lock()
        values.append(value)
        lock.unlock()
    }

    var all: [Double] {
        lock.lock()
        defer { lock.unlock() }
        return values
    }
}

/// Swift → UniFFI → Rust → a real companion server. `make swift-test` starts one and sets these.
final class EndToEndTests: XCTestCase {
    private var service: CompanionService!
    private var directory: URL!
    private var settings: ServerSettings!

    override func setUpWithError() throws {
        let env = ProcessInfo.processInfo.environment
        guard let url = env["COMPANION_E2E_URL"], let id = env["COMPANION_E2E_ID"], let secret = env["COMPANION_E2E_SECRET"] else {
            throw XCTSkip("no server: run `make swift-test`")
        }
        settings = ServerSettings(url: url, clientId: id)
        service = try CompanionService(settings: settings, secret: secret)
        directory = FileManager.default.temporaryDirectory.appendingPathComponent("companion-kit-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        if let directory {
            try? FileManager.default.removeItem(at: directory)
        }
    }

    func testUploadListDownload() async throws {
        var content = Data((0..<2_000_000).map { UInt8($0 % 251) })
        content.append(Data("\r\n--boundary--\r\n".utf8))
        let source = directory.appendingPathComponent("source.bin")
        try content.write(to: source)

        let name = "photo \(UUID().uuidString).jpg"
        let sent = Fractions()
        let uploaded = try await service.upload(fileAt: source, named: name) { sent.add($0) }
        XCTAssertEqual(uploaded.filename, name)
        XCTAssertEqual(uploaded.size, UInt64(content.count))
        XCTAssertEqual(sent.all.last, 1.0)
        XCTAssertEqual(sent.all, sent.all.sorted(), "progress goes forward")
        XCTAssertLessThanOrEqual(sent.all.count, 101, "at most once per percent")

        let files = try await service.files()
        XCTAssertEqual(files.first?.id, uploaded.id, "newest first")
        let file = try XCTUnwrap(files.first)
        XCTAssertEqual(file.mimetype, "image/jpeg")
        XCTAssertNotNil(file.uploadedDate)

        let received = Fractions()
        let local = try await service.download(file, into: directory) { received.add($0) }
        XCTAssertEqual(try Data(contentsOf: local), content)
        XCTAssertEqual(received.all.last, 1.0)

        // Already there in full: not fetched again.
        let again = Fractions()
        let cached = try await service.download(file, into: directory) { again.add($0) }
        XCTAssertEqual(cached, local)
        XCTAssertEqual(again.all, [])
    }

    func testPadAndPreview() async throws {
        let first = try await service.setPad("one")
        try await service.appendToPad("https://example.com/shared")
        let second = try await service.appendToPad("three — é 🙂")
        let pad = try await service.pad()
        XCTAssertEqual(pad.content, "one\nhttps://example.com/shared\nthree — é 🙂\n")
        XCTAssertEqual(pad.timestamp, second)
        XCTAssertGreaterThan(second, first)

        let uploaded = try await service.upload(data: Data("%PDF".utf8), named: "slides.pdf")
        try await service.present(fileId: uploaded.id)
        let preview = try await service.preview()
        XCTAssertEqual(preview.fileId, uploaded.id)
        XCTAssertEqual(preview.filename, "slides.pdf")
    }

    func testErrorsReachSwiftTyped() async throws {
        let wrong = try CompanionService(settings: settings, secret: "ffff")
        do {
            _ = try await wrong.files()
            XCTFail("a wrong secret was accepted")
        } catch let error as CompanionError {
            XCTAssertEqual(error, .Unauthorized)
            XCTAssertEqual(message(for: error), "invalid credentials")
        }

        let nowhere = try CompanionService(settings: ServerSettings(url: "http://127.0.0.1:9", clientId: "a"), secret: "ab")
        do {
            _ = try await nowhere.files()
            XCTFail("nothing listens there")
        } catch let error as CompanionError {
            guard case .Network = error else { return XCTFail("\(error)") }
            XCTAssertTrue(message(for: error).contains("127.0.0.1"), message(for: error))
        }
    }
}
