import Foundation
import XCTest
@testable import BankKit

let pinA = String(repeating: "ab", count: 32)
let pinB = String(repeating: "cd", count: 32)

/// A store of its own for a test, with nothing of the real keychain.
func makeStore(_ test: XCTestCase) -> SettingsStore {
    let suite = "bank-tests-\(UUID().uuidString)"
    let defaults = UserDefaults(suiteName: suite)!
    test.addTeardownBlock { defaults.removePersistentDomain(forName: suite) }
    return SettingsStore(defaults: defaults, secrets: InMemorySecretStore())
}

func makeDirectory(_ test: XCTestCase) throws -> URL {
    let directory = FileManager.default.temporaryDirectory.appendingPathComponent("bank-kit-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    test.addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
    return directory
}

/// Remembers which URLs a certificate was read from: nothing is connected to.
final class CertificateReads: @unchecked Sendable {
    private let lock = NSLock()
    private var urls: [String] = []
    var answer = CertificateFingerprint(sha256: pinA, chainValid: true)

    func read(_ url: String) -> CertificateFingerprint {
        lock.lock()
        defer { lock.unlock() }
        urls.append(url)
        return answer
    }

    var all: [String] {
        lock.lock()
        defer { lock.unlock() }
        return urls
    }
}

final class SettingsTests: XCTestCase {
    func testStoreRoundTrip() throws {
        let store = makeStore(self)
        XCTAssertEqual(store.load().settings, BankSettings())
        XCTAssertEqual(store.load().credentials, Credentials())
        XCTAssertEqual(BankSettings().attachmentsHost, "qonto.s3.eu-central-1.amazonaws.com")
        XCTAssertFalse(BankSettings().isComplete, "no pin, no connection")

        let settings = BankSettings(apiPins: [pinA], attachmentsPins: [pinA, pinB], selectedAccountId: "id")
        let credentials = Credentials(organizationSlug: "my-org-1234", secretKey: "s3cr3t")
        try store.save(settings)
        XCTAssertEqual(store.load().credentials, Credentials(), "saving the settings touches no secret")
        try store.save(credentials)
        XCTAssertEqual(store.load().settings, settings)
        XCTAssertEqual(store.load().credentials, credentials)

        try store.save(Credentials())
        XCTAssertEqual(store.load().credentials, Credentials())
        XCTAssertEqual(store.load().settings, settings, "the pins outlive the credentials")
    }

    func testCredentialsNeverReachTheSettings() throws {
        let suite = "bank-tests-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = SettingsStore(defaults: defaults, secrets: InMemorySecretStore())
        try store.save(BankSettings(apiPins: [pinA]))
        try store.save(Credentials(organizationSlug: "my-org-1234", secretKey: "s3cr3t-key"))
        for (key, value) in defaults.persistentDomain(forName: suite) ?? [:] {
            let text = (value as? Data).map { String(decoding: $0, as: UTF8.self) } ?? "\(value)"
            XCTAssertFalse(text.contains("s3cr3t-key") || text.contains("my-org-1234"), key)
        }
    }

    func testTheRealClientNeedsAPin() {
        let credentials = Credentials(organizationSlug: "my-org", secretKey: "secret")
        XCTAssertThrowsError(try BankService(settings: BankSettings(), credentials: credentials)) {
            XCTAssertEqual(message(for: $0), "no certificate is pinned for the Qonto API")
        }
        XCTAssertNoThrow(try BankService(settings: BankSettings(apiPins: [pinA]), credentials: credentials))
        // The door for tests only opens on this machine.
        XCTAssertThrowsError(try BankService(mockAt: "https://thirdparty.qonto.com", credentials: credentials))
        XCTAssertThrowsError(try BankService(mockAt: "http://192.168.1.10:8080", credentials: credentials))
    }

    func testPinsOfASettings() throws {
        var settings = BankSettings()
        try settings.addPins([pinA, pinB, pinA], for: .api)
        try settings.addPins([pinB], for: .attachments)
        XCTAssertEqual(settings.pins(for: .api), [pinA, pinB])
        XCTAssertEqual(settings.pins(for: .attachments), [pinB])
        settings.removePin(pinA, for: .api)
        XCTAssertEqual(settings.apiPins, [pinB])

        let many = (0..<maxPins).map { String(repeating: "0", count: 62) + String(format: "%02x", $0) }
        try settings.addPins(Array(many.dropLast()), for: .api)
        XCTAssertEqual(settings.apiPins.count, maxPins)
        XCTAssertThrowsError(try settings.addPins([many.last!], for: .api)) {
            XCTAssertEqual($0 as? PinError, .tooMany)
        }
        XCTAssertEqual(settings.apiPins.count, maxPins, "all of them or none")
    }

    func testWhereCertificatesAreReadFrom() throws {
        var settings = BankSettings()
        XCTAssertEqual(try settings.certificateURL(for: .api), "https://thirdparty.qonto.com")
        XCTAssertEqual(try settings.certificateURL(for: .attachments), "https://qonto.s3.eu-central-1.amazonaws.com")
        settings.attachmentsHost = " Bucket.S3.eu-west-3.amazonaws.com "
        XCTAssertEqual(try settings.certificateURL(for: .attachments), "https://bucket.s3.eu-west-3.amazonaws.com")
        XCTAssertEqual(try settings.certificateURL(for: .api), "https://thirdparty.qonto.com", "not a setting")
        for bad in ["", "localhost", "https://a.example", "a.example/path", "a.example:443", "user@a.example",
                    "a..example", "-a.example", "a.exa mple", "é.example"] {
            settings.attachmentsHost = bad
            XCTAssertThrowsError(try settings.certificateURL(for: .attachments), bad)
        }
    }

    @MainActor
    func testPinsGetOntoThePhone() async throws {
        let store = makeStore(self)
        let reads = CertificateReads()
        let environment = BankEnvironment(
            makeService: { try BankService(settings: $0, credentials: $1) },
            readFingerprint: { reads.read($0) })
        let model = AppModel(store: store, downloadDirectory: try makeDirectory(self), environment: environment)
        XCTAssertFalse(model.isConnected)

        // 1. Read the served certificate, compare, trust.
        let offered = await model.readCertificate(for: .api)
        let offer = try XCTUnwrap(offered)
        XCTAssertEqual(reads.all, ["https://thirdparty.qonto.com"])
        XCTAssertEqual(offer, PinOffer(
            target: .api, host: "thirdparty.qonto.com", sha256: pinA, chainValid: true, alreadyPinned: false))
        XCTAssertTrue(offer.canBePinned)
        XCTAssertEqual(model.settings.apiPins, [], "reading pins nothing")
        XCTAssertTrue(model.trust(offer))
        XCTAssertEqual(model.settings.apiPins, [pinA])
        XCTAssertEqual(store.load().settings.apiPins, [pinA], "saved")
        let again = await model.readCertificate(for: .api)
        XCTAssertEqual(again?.alreadyPinned, true)
        XCTAssertEqual(again?.canBePinned, false)

        // A certificate that the public CAs do not vouch for is never pinned.
        reads.answer = CertificateFingerprint(sha256: pinB, chainValid: false)
        let intercepted = await model.readCertificate(for: .attachments)
        let bad = try XCTUnwrap(intercepted)
        XCTAssertEqual(reads.all.last, "https://qonto.s3.eu-central-1.amazonaws.com")
        XCTAssertFalse(bad.canBePinned)
        XCTAssertFalse(model.trust(bad))
        XCTAssertEqual(model.settings.attachmentsPins, [])
        XCTAssertEqual(model.errorMessage, PinError.notVerifiable.errorDescription)

        // A host that is not one is not connected to.
        model.setAttachmentsHost("evil.example/x")
        let none = await model.readCertificate(for: .attachments)
        XCTAssertNil(none)
        XCTAssertEqual(reads.all.count, 3)
        XCTAssertEqual(model.errorMessage, "Not a host name: evil.example/x")

        // 2. Paste what tls-pin-get prints: all of it or nothing.
        let colons = Array(repeating: "CD", count: 32).joined(separator: ":")
        XCTAssertTrue(model.addPins(pasted: "\(colons)\n\(pinA)\n", for: .attachments))
        XCTAssertEqual(model.settings.attachmentsPins, [pinB, pinA])
        model.errorMessage = nil
        XCTAssertFalse(model.addPins(pasted: "\(pinA)\nnot a pin\n", for: .api))
        XCTAssertEqual(model.errorMessage, "Not a SHA-256: not a pin")
        XCTAssertFalse(model.addPins(pasted: "", for: .api))
        XCTAssertEqual(model.settings.apiPins, [pinA])

        model.removePin(pinB, for: .attachments)
        XCTAssertEqual(store.load().settings.attachmentsPins, [pinA])

        // With a pin and credentials, there is a client; without the pin, there is none.
        try store.save(Credentials(organizationSlug: "my-org", secretKey: "secret"))
        let restarted = AppModel(store: store, downloadDirectory: try makeDirectory(self), environment: environment)
        XCTAssertTrue(restarted.isConnected)
        restarted.removePin(pinA, for: .api)
        XCTAssertFalse(restarted.isConnected)
    }

    func testFingerprintGrouping() {
        XCTAssertEqual(groupedFingerprint("ab12cd34ef"), "ab12 cd34 ef")
        XCTAssertEqual(groupedFingerprint(""), "")
    }
}

final class PresentationTests: XCTestCase {
    /// The spaces a locale puts in an amount are not the one of the keyboard.
    private func plain(_ text: String) -> String {
        String(text.unicodeScalars.map { $0.properties.isWhitespace ? " " : Character($0) })
    }

    func testMoneyIsExactAndFormattedAtTheEdge() {
        let debit = Money(cents: -123_456, currency: "EUR")
        XCTAssertEqual(debit.decimal, Decimal(string: "-1234.56"))
        XCTAssertEqual(plain(debit.formatted(locale: Locale(identifier: "fr_FR"))), "-1 234,56 €")
        XCTAssertEqual(debit.formatted(locale: Locale(identifier: "en_US")), "-€1,234.56")
        XCTAssertEqual(Money(cents: 9999, currency: "USD").formatted(locale: Locale(identifier: "en_US")), "$99.99")
        XCTAssertEqual(Money(cents: 5, currency: "EUR").formatted(locale: Locale(identifier: "en_US")), "€0.05")
        XCTAssertEqual(Money(cents: 0, currency: "EUR").decimal, 0)
        // What a Double cannot hold: every cent is still there.
        let large = Money(cents: 9_007_199_254_740_993, currency: "EUR")
        XCTAssertEqual(large.decimal, Decimal(string: "90071992547409.93"))
        XCTAssertEqual(large.formatted(locale: Locale(identifier: "en_US")), "€90,071,992,547,409.93")
        XCTAssertEqual(Money(cents: .min, currency: "EUR").decimal, Decimal(string: "-92233720368547758.08"))
    }

    private func transaction(side: String = "debit", localCurrency: String? = nil) -> Transaction {
        Transaction(
            transactionId: "my-org-1-transaction-123456", id: "00000000-0000-0000-0000-000000000001",
            label: "Hotel", amountCents: 9150, localAmountCents: 9999, currency: "EUR",
            localCurrency: localCurrency, side: side, emittedAt: "2024-06-15T10:30:00.000Z",
            settledAt: "2024-06-17T00:00:00Z", status: nil, operationType: "card", cleanCounterpartyName: "HOTEL",
            reference: nil, note: nil, cardLastDigits: nil, attachmentIds: [])
    }

    func testTransactions() {
        let abroad = transaction(localCurrency: "USD")
        XCTAssertEqual(abroad.amount, Money(cents: -9150, currency: "EUR"), "a debit leaves the account")
        XCTAssertEqual(abroad.localAmount, Money(cents: -9999, currency: "USD"))
        XCTAssertEqual(abroad.shortId, "123456")
        XCTAssertEqual(abroad.symbolName, "creditcard")
        XCTAssertFalse(abroad.hasAttachments)
        XCTAssertEqual(abroad.emittedDate, Date(timeIntervalSince1970: 1_718_447_400))
        XCTAssertEqual(abroad.settledDate, Date(timeIntervalSince1970: 1_718_582_400))

        XCTAssertEqual(transaction(side: "credit").amount, Money(cents: 9150, currency: "EUR"))
        XCTAssertNil(transaction().localAmount)
        XCTAssertNil(transaction(localCurrency: "EUR").localAmount, "the same currency is not a foreign one")

        var unnamed = transaction()
        unnamed.label = ""
        XCTAssertEqual(unnamed.title, "HOTEL")
        unnamed.cleanCounterpartyName = nil
        XCTAssertEqual(unnamed.title, "123456")
        unnamed.emittedAt = "yesterday"
        XCTAssertNil(unnamed.emittedDate)
    }

    func testAccounts() {
        var account = BankAccount(
            id: "12345678-1234-1234-1234-123456789abc", name: "Main", iban: "FR7616958000010000000000000",
            currency: "EUR", balanceCents: 1_234_567, authorizedBalanceCents: 1_200_000, status: "active")
        XCTAssertEqual(account.displayName, "Main")
        XCTAssertEqual(account.balance, Money(cents: 1_234_567, currency: "EUR"))
        XCTAssertEqual(account.authorizedBalance, Money(cents: 1_200_000, currency: "EUR"))
        XCTAssertEqual(account.groupedIBAN, "FR76 1695 8000 0100 0000 0000 000")
        account.authorizedBalanceCents = 1_234_567
        XCTAssertNil(account.authorizedBalance, "the same as the balance says nothing more")
        (account.name, account.balanceCents) = (nil, nil)
        XCTAssertEqual(account.displayName, "FR7616958000010000000000000")
        XCTAssertNil(account.balance, "no balance is not a balance of zero")
        account.iban = nil
        XCTAssertEqual(account.displayName, "Account …789abc")
    }

    func testPeriodsAreWhatTheCLISends() {
        XCTAssertEqual(ListFilter().transactionFilter, TransactionFilter(
            withAttachments: nil, emittedAtFrom: nil, emittedAtTo: nil))
        // bank transactions no-invoice date=2024
        XCTAssertEqual(ListFilter(missingReceiptOnly: true, period: .year(2024)).transactionFilter, TransactionFilter(
            withAttachments: false, emittedAtFrom: "2024-01-01T00:00:00Z", emittedAtTo: "2024-12-31T23:59:59Z"))
        let march = Period.days(from: Day(year: 2024, month: 3, day: 1), to: Day(year: 2024, month: 3, day: 31))
        XCTAssertEqual(ListFilter(period: march).transactionFilter, TransactionFilter(
            withAttachments: nil, emittedAtFrom: "2024-03-01T00:00:00Z", emittedAtTo: "2024-03-31T23:59:59Z"))
        let since = Period.days(from: Day(year: 987, month: 12, day: 5), to: nil)
        XCTAssertEqual(ListFilter(period: since).transactionFilter.emittedAtFrom, "0987-12-05T00:00:00Z")
        XCTAssertNil(ListFilter(period: since).transactionFilter.emittedAtTo)

        XCTAssertTrue(march.isValid)
        XCTAssertTrue(since.isValid)
        XCTAssertTrue(Period.days(from: Day(year: 2024, month: 3, day: 1), to: Day(year: 2024, month: 3, day: 1)).isValid)
        XCTAssertFalse(Period.days(from: Day(year: 2024, month: 3, day: 2), to: Day(year: 2024, month: 3, day: 1)).isValid)

        let day = Day(year: 2024, month: 3, day: 1)
        XCTAssertEqual(Period.all.title, "All dates")
        XCTAssertEqual(Period.year(2024).title, "2024")
        XCTAssertEqual(march.title, "2024-03-01 – 2024-03-31")
        XCTAssertEqual(Period.days(from: day, to: day).title, "2024-03-01")
        XCTAssertEqual(Period.days(from: nil, to: day).title, "Until 2024-03-01")
        XCTAssertEqual(since.title, "Since 0987-12-05")

        // The day is the one the user sees on the date picker, wherever they are.
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "Pacific/Auckland")!
        let newYearInAuckland = Date(timeIntervalSince1970: 1_704_020_400) // 2023-12-31T11:00:00Z
        XCTAssertEqual(Day(newYearInAuckland, calendar: calendar), Day(year: 2024, month: 1, day: 1))
        XCTAssertEqual(Period.calendarYear(now: newYearInAuckland, calendar: calendar), .year(2024))
        XCTAssertEqual(Period.calendarYear(yearsAgo: 1, now: newYearInAuckland, calendar: calendar), .year(2023))
    }

    func testReceiptsAreWhatTheirContentIs() throws {
        let directory = try makeDirectory(self)
        let jpeg = Data([0xff, 0xd8, 0xff, 0xe0]) + Data("photo".utf8)
        let heic = Data("\0\0\0\u{18}ftypheic".utf8)
        XCTAssertEqual(receiptType(of: jpeg)?.mimetype, "image/jpeg")
        XCTAssertEqual(receiptType(of: Data("%PDF-1.7".utf8))?.extension, "pdf")
        XCTAssertNil(receiptType(of: heic))
        XCTAssertNil(receiptType(of: Data()))

        // What Qonto accepts is kept byte for byte, and never converted.
        let kept = try prepareReceipt(jpeg, in: directory) { _ in XCTFail("converted"); return nil }
        XCTAssertEqual(kept.lastPathComponent, "receipt.jpg")
        XCTAssertEqual(try Data(contentsOf: kept), jpeg)
        XCTAssertEqual(receiptType(ofFileAt: kept)?.extension, "jpg")
        // What it does not goes through the converter of the app.
        let converted = try prepareReceipt(heic, in: directory) { _ in jpeg }
        XCTAssertEqual(try Data(contentsOf: converted), jpeg)
        XCTAssertNotEqual(converted, kept, "each receipt has a file of its own")
        // And is refused when that gives nothing usable.
        for convert in [{ _ in nil }, { _ in Data("GIF89a".utf8) }] as [(Data) -> Data?] {
            XCTAssertThrowsError(try prepareReceipt(heic, in: directory, convert: convert)) {
                XCTAssertEqual($0 as? BankError, .UnsupportedFile)
            }
        }
        XCTAssertNil(receiptType(ofFileAt: directory.appendingPathComponent("missing")))
    }
}

/// Swift → UniFFI → Rust → the mock of the Qonto API. `swift-test.sh` starts one and sets these.
/// Its data: crates/bank-core/tests/mock/mod.rs, `State::sample`, two transactions per page.
@MainActor
final class EndToEndTests: XCTestCase {
    private let mainAccount = "12345678-1234-1234-1234-123456789abc"
    private let otherAccount = "12345678-1234-1234-1234-123456789abd"
    private var url = ""
    private var credentials = Credentials()
    private var store: SettingsStore!
    private var directory: URL!
    private var model: AppModel!

    private func makeModel() -> AppModel {
        let url = url
        let environment = BankEnvironment(
            makeService: { _, credentials in try BankService(mockAt: url, credentials: credentials) },
            readFingerprint: { _ in throw BankError.Network(message: "not in this test") })
        return AppModel(store: store, downloadDirectory: directory, environment: environment)
    }

    override func setUp() async throws {
        let env = ProcessInfo.processInfo.environment
        guard let url = env["BANK_E2E_URL"], let slug = env["BANK_E2E_SLUG"], let secret = env["BANK_E2E_SECRET"] else {
            throw XCTSkip("no mock server: run apps/bank/swift-test.sh")
        }
        self.url = url
        credentials = Credentials(organizationSlug: slug, secretKey: secret)
        store = makeStore(self)
        // The mock is over http and takes no pin: this one only makes the settings complete.
        try store.save(BankSettings(apiPins: [pinA]))
        directory = try makeDirectory(self)
        model = makeModel()
    }

    private var titles: [String] { model.transactions.map(\.title) }

    func testConnectAccountsAndPages() async throws {
        XCTAssertFalse(model.isConnected)
        let connected = await model.connect(credentials)
        XCTAssertTrue(connected, model.errorMessage ?? "")
        XCTAssertTrue(model.isConnected)
        XCTAssertEqual(store.load().credentials, credentials, "saved once they worked")

        XCTAssertEqual(model.accounts.map(\.displayName), ["Main account", "Taxes"])
        XCTAssertEqual(model.selectedAccount?.id, mainAccount, "the first one, as the CLI")
        XCTAssertEqual(model.selectedAccount?.balance, Money(cents: 1_234_567, currency: "EUR"))
        XCTAssertEqual(model.accounts[1].balance, Money(cents: 5, currency: "EUR"))

        // Connecting lists the first page.
        XCTAssertEqual(titles, ["Hotel California", "label 4"])
        XCTAssertEqual(model.totalCount, 5)
        XCTAssertTrue(model.hasMoreTransactions)
        XCTAssertFalse(model.isLoadingTransactions)
        XCTAssertEqual(model.transactions[0].amount, Money(cents: -9150, currency: "EUR"))
        XCTAssertEqual(model.transactions[0].localAmount, Money(cents: -9999, currency: "USD"))
        XCTAssertEqual(model.attachmentCount(of: model.transactions[1]), 1)

        // The first row on screen asks for nothing, the last one for the next page.
        await model.loadMoreIfNeeded(after: model.transactions[0])
        XCTAssertEqual(titles.count, 2)
        await model.loadMoreIfNeeded(after: model.transactions[1])
        XCTAssertEqual(titles, ["Hotel California", "label 4", "label 3", "label 2"])
        await model.loadMoreTransactions()
        XCTAssertEqual(titles, ["Hotel California", "label 4", "label 3", "label 2", "label 1"])
        XCTAssertFalse(model.hasMoreTransactions)
        await model.loadMoreTransactions()
        XCTAssertEqual(titles.count, 5)
        XCTAssertEqual(model.transactions[2].amount, Money(cents: 250_000, currency: "EUR"), "a credit")

        // Pulling to refresh starts from the first page again.
        await model.refresh()
        XCTAssertEqual(titles, ["Hotel California", "label 4"])
        XCTAssertNil(model.errorMessage)

        // The other account, remembered at the next launch.
        await model.select(model.accounts[1])
        XCTAssertEqual(titles, ["label 6"])
        XCTAssertFalse(model.hasMoreTransactions)
        let relaunched = makeModel()
        XCTAssertTrue(relaunched.isConnected)
        await relaunched.refresh()
        XCTAssertEqual(relaunched.selectedAccount?.id, otherAccount)
        XCTAssertEqual(relaunched.transactions.map(\.title), ["label 6"])
    }

    func testWrongCredentialsAreNotSaved() async throws {
        let wrong = Credentials(organizationSlug: credentials.organizationSlug, secretKey: "wrong")
        let connected = await model.connect(wrong)
        XCTAssertFalse(connected)
        XCTAssertEqual(model.errorMessage, "HTTP 401 Unauthorized: unauthorized: Invalid credentials")
        XCTAssertFalse(model.isConnected)
        XCTAssertEqual(store.load().credentials, Credentials())
        XCTAssertNil(model.pinProblem)

        // What cannot go in a header is refused before anything is sent.
        let odd = await model.connect(Credentials(organizationSlug: "my org", secretKey: "x"))
        XCTAssertFalse(odd)
        XCTAssertEqual(model.errorMessage, "the organization slug is letters, digits and hyphens, as Qonto shows it")

        // Errors reach Swift typed.
        let service = try BankService(mockAt: url, credentials: wrong)
        do {
            _ = try await service.accounts()
            XCTFail("a wrong key was accepted")
        } catch let error as BankError {
            guard case .Http(let status, _) = error else { return XCTFail("\(error)") }
            XCTAssertEqual(status, 401)
        }
    }

    func testFilters() async throws {
        await model.connect(credentials)

        await model.setFilter(ListFilter(missingReceiptOnly: true))
        XCTAssertEqual(titles, ["Hotel California", "label 3"])
        XCTAssertEqual(model.totalCount, 3)
        await model.loadMoreTransactions()
        XCTAssertEqual(titles, ["Hotel California", "label 3", "label 1"])

        await model.setFilter(ListFilter(period: .year(2023)))
        XCTAssertEqual(titles, ["label 2", "label 1"])
        XCTAssertFalse(model.hasMoreTransactions)

        await model.setFilter(ListFilter(missingReceiptOnly: true, period: .year(2024)))
        XCTAssertEqual(titles, ["Hotel California", "label 3"])
        let march = Period.days(from: Day(year: 2024, month: 2, day: 1), to: Day(year: 2024, month: 3, day: 1))
        await model.setFilter(ListFilter(period: march))
        XCTAssertEqual(titles, ["label 4"])
        XCTAssertNil(model.errorMessage)

        // A range that ends before it starts is refused by the core: nothing is listed, and it is said.
        let backwards = Period.days(from: Day(year: 2024, month: 3, day: 2), to: Day(year: 2024, month: 3, day: 1))
        await model.setFilter(ListFilter(period: backwards))
        XCTAssertEqual(titles, [])
        XCTAssertEqual(model.errorMessage, "Invalid date range: 'from' date must be before 'to' date")
    }

    func testAnAnswerToAFilterThatChangedIsDropped() async throws {
        await model.connect(credentials)
        let model = try XCTUnwrap(self.model)
        // Two filters asked one right after the other: whichever answer comes last, the list is the one
        // of the filter that is shown.
        let first = Task { await model.setFilter(ListFilter(period: .year(2023))) }
        let second = Task { await model.setFilter(ListFilter(missingReceiptOnly: true)) }
        await first.value
        await second.value
        XCTAssertFalse(model.isLoadingTransactions)
        if model.filter.missingReceiptOnly {
            XCTAssertEqual(titles, ["Hotel California", "label 3"])
        } else {
            XCTAssertEqual(titles, ["label 2", "label 1"])
        }
    }

    func testAttachmentsAreListedAndDownloaded() async throws {
        await model.connect(credentials)
        await model.loadMoreTransactions()
        let withTwo = try XCTUnwrap(model.transactions.first { $0.title == "label 2" })
        XCTAssertNil(model.attachments[withTwo.id])
        await model.loadAttachments(of: withTwo)
        let listed = try XCTUnwrap(model.attachments[withTwo.id])
        XCTAssertEqual(listed.map(\.symbolName), ["doc.richtext", "photo"])

        let pdf = await model.localCopy(of: listed[0], in: withTwo)
        let pdfURL = try XCTUnwrap(pdf, model.errorMessage ?? "")
        XCTAssertEqual(pdfURL.deletingLastPathComponent().path, directory.path)
        XCTAssertEqual(pdfURL.lastPathComponent, "\(listed[0].id).pdf", "an extension Quick Look can go by")
        XCTAssertEqual(try Data(contentsOf: pdfURL), Data("%PDF-1.7\n%mock invoice\n".utf8))
        XCTAssertEqual(model.downloads, [:])

        // A URL that does not work anymore (they only last a while): a new one is asked for.
        var expired = listed[1]
        expired.url = expired.url?.replacingOccurrences(of: "X-Amz-Signature=mock", with: "X-Amz-Signature=old")
        let png = await model.localCopy(of: expired, in: withTwo)
        XCTAssertEqual(png?.pathExtension, "png", model.errorMessage ?? "")
        XCTAssertNil(model.errorMessage)

        model.clearDownloads()
        XCTAssertFalse(FileManager.default.fileExists(atPath: pdfURL.path))
    }

    /// The only test that changes what the mock holds: transaction 6, alone on the other account.
    func testAReceiptIsAttachedOnce() async throws {
        await model.connect(credentials)
        await model.select(model.accounts[1])
        await model.setFilter(ListFilter(missingReceiptOnly: true))
        XCTAssertEqual(titles, ["label 6"])
        let transaction = model.transactions[0]

        let content = Data("%PDF-1.4 receipt \(UUID().uuidString)".utf8)
        let receipt = { () throws -> URL in
            try prepareReceipt(content, in: self.directory) { _ in nil }
        }

        // Not a receipt: said, and not sent.
        let gif = directory.appendingPathComponent("animation.pdf")
        try Data("GIF89a".utf8).write(to: gif)
        let refused = await model.attach(fileAt: gif, to: transaction)
        XCTAssertFalse(refused)
        XCTAssertEqual(
            model.errorMessage,
            "Unsupported file type: only PDF, JPEG and PNG are accepted (checked on the first bytes)")
        model.errorMessage = nil

        let first = try receipt()
        let sent = await model.attach(fileAt: first, to: transaction, cleanUp: true)
        XCTAssertTrue(sent, model.errorMessage ?? "")
        XCTAssertFalse(FileManager.default.fileExists(atPath: first.path), "the temporary copy is removed")
        XCTAssertEqual(model.uploads, [])
        XCTAssertEqual(model.attachmentCount(of: transaction), 1)
        XCTAssertEqual(titles, [], "it has a receipt now: not in the list of the ones that miss one")
        XCTAssertEqual(model.totalCount, 0)

        // The same file again: nothing is sent, the user is asked.
        let second = try receipt()
        let again = await model.attach(fileAt: second, to: transaction, cleanUp: true)
        XCTAssertFalse(again)
        XCTAssertEqual(model.pendingReceipt?.reason, .alreadyThere)
        XCTAssertNil(model.errorMessage)
        XCTAssertEqual(model.attachmentCount(of: transaction), 1)
        XCTAssertTrue(FileManager.default.fileExists(atPath: second.path), "kept while the user decides")
        // "Cancel"
        model.dropPendingReceipt()
        XCTAssertNil(model.pendingReceipt)
        XCTAssertFalse(FileManager.default.fileExists(atPath: second.path))

        // "Send anyway"
        let third = try receipt()
        _ = await model.attach(fileAt: third, to: transaction, cleanUp: true)
        XCTAssertEqual(model.pendingReceipt?.reason, .alreadyThere)
        let forced = await model.sendPendingReceipt()
        XCTAssertTrue(forced, model.errorMessage ?? "")
        XCTAssertNil(model.pendingReceipt)
        XCTAssertEqual(model.attachmentCount(of: transaction), 2)
        XCTAssertFalse(FileManager.default.fileExists(atPath: third.path))

        // What was sent is what is there.
        let attachment = try XCTUnwrap(model.attachments[transaction.id]?.first)
        let local = await model.localCopy(of: attachment, in: transaction)
        XCTAssertEqual(try Data(contentsOf: try XCTUnwrap(local)), content)

        // Signing out leaves no key, no data and no file. The pins stay.
        model.signOut()
        XCTAssertFalse(model.isConnected)
        XCTAssertEqual(store.load().credentials, Credentials())
        XCTAssertEqual(store.load().settings.apiPins, [pinA])
        XCTAssertEqual(model.accounts, [])
        XCTAssertEqual(model.attachments, [:])
        XCTAssertFalse(FileManager.default.fileExists(atPath: directory.path))
    }
}
