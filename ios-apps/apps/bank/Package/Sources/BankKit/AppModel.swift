import Foundation
import Observation

/// A receipt on its way to a transaction.
public struct UploadInProgress: Identifiable, Equatable, Sendable {
    public let id: UUID
    /// The id (UUID) of the transaction.
    public let transactionId: String
    public var fraction: Double
}

/// A receipt that was not sent, and can still be: the user decides.
public struct PendingReceipt: Equatable, Sendable {
    public enum Reason: Equatable, Sendable {
        /// The transaction already has this very file.
        case alreadyThere
        /// What the transaction has could not be checked (the message says why), so nothing was sent.
        case notChecked(String)
    }

    public let transaction: Transaction
    public let file: URL
    public let reason: Reason
    let cleanUp: Bool
}

/// Everything the views show, and everything they can ask for. No view talks to the API itself.
@MainActor
@Observable
public final class AppModel {
    public private(set) var settings: BankSettings
    public private(set) var credentials: Credentials
    /// `nil` until there are credentials and a pin for the API.
    public private(set) var service: BankService?

    public private(set) var accounts: [BankAccount] = []
    public private(set) var filter = ListFilter()
    /// Of the selected account, for `filter`, newest first: the pages loaded so far.
    public private(set) var transactions: [Transaction] = []
    /// How many transactions match in all, when the API says.
    public private(set) var totalCount: UInt64?
    public private(set) var isLoadingTransactions = false
    /// What each transaction has, by its id (UUID), once it was asked for.
    public private(set) var attachments: [String: [Attachment]] = [:]

    public private(set) var uploads: [UploadInProgress] = []
    /// Attachment id → fraction, while it downloads.
    public private(set) var downloads: [String: Double] = [:]

    /// A receipt waiting for "send anyway" or "cancel".
    public private(set) var pendingReceipt: PendingReceipt?
    /// A certificate that is not pinned was served: nothing goes through until Settings deals with it.
    public private(set) var pinProblem: PinProblem?
    /// The last failure, for an alert. Set to `nil` to dismiss.
    public var errorMessage: String?

    private let store: SettingsStore
    private let downloadDirectory: URL
    private let environment: BankEnvironment
    private var nextPage: UInt32?
    /// Changes whenever the list is started over: what an older request answers is dropped.
    private var generation = 0

    public init(store: SettingsStore, downloadDirectory: URL, environment: BankEnvironment = .production) {
        self.store = store
        self.downloadDirectory = downloadDirectory
        self.environment = environment
        (settings, credentials) = store.load()
        rebuildService()
    }

    public var isConnected: Bool { service != nil }
    public var hasMoreTransactions: Bool { nextPage != nil }

    /// The account whose transactions are listed: the one that was chosen, or the first one, as the CLI.
    public var selectedAccount: BankAccount? {
        accounts.first { $0.id == settings.selectedAccountId } ?? accounts.first
    }

    private func fail(_ error: Error) {
        if case let BankError.PinMismatch(which, host, served) = error {
            pinProblem = PinProblem(target: PinTarget(which), host: host, served: served)
        }
        errorMessage = message(for: error)
    }

    private func rebuildService() {
        let usable = settings.isComplete && credentials.isComplete
        service = usable ? try? environment.makeService(settings, credentials) : nil
    }

    // MARK: Credentials

    /// Checks the credentials against the API (lists the accounts) and only then saves them.
    /// Returns whether it worked; the reason is in `errorMessage` when it did not.
    @discardableResult
    public func connect(_ newCredentials: Credentials) async -> Bool {
        do {
            let candidate = try environment.makeService(settings, newCredentials)
            let listed = try await candidate.accounts()
            try store.save(newCredentials)
            if newCredentials.organizationSlug != credentials.organizationSlug {
                // Another organization: nothing of the previous one applies.
                forgetData()
                try updateSettings { $0.selectedAccountId = nil }
            }
            (credentials, service, accounts) = (newCredentials, candidate, listed)
            await refreshTransactions()
            return true
        } catch {
            fail(error)
            return false
        }
    }

    /// Removes the credentials from the keychain, and what was downloaded. The pins stay: they are not secret.
    public func signOut() {
        do {
            try store.save(Credentials())
            credentials = Credentials()
            service = nil
            forgetData()
            clearDownloads()
        } catch {
            fail(error)
        }
    }

    private func forgetData() {
        generation += 1
        accounts = []
        transactions = []
        attachments = [:]
        (nextPage, totalCount, pendingReceipt) = (nil, nil, nil)
    }

    // MARK: Pins

    /// Saves a change of the settings, and applies it.
    private func updateSettings(_ change: (inout BankSettings) throws -> Void) throws {
        var changed = settings
        try change(&changed)
        guard changed != settings else { return }
        try store.save(changed)
        let pinsChanged = PinTarget.allCases.filter { changed.pins(for: $0) != settings.pins(for: $0) }
        settings = changed
        if !pinsChanged.isEmpty {
            rebuildService()
            if let problem = pinProblem, pinsChanged.contains(problem.target) {
                pinProblem = nil
            }
        }
    }

    /// Reads the certificate that a host serves, to show it before it is pinned. Sends it nothing.
    public func readCertificate(for target: PinTarget) async -> PinOffer? {
        do {
            let url = try settings.certificateURL(for: target)
            let read = environment.readFingerprint
            let found = try await offMain { try read(url) }
            return PinOffer(
                target: target, host: settings.host(for: target), sha256: found.sha256,
                chainValid: found.chainValid, alreadyPinned: settings.pins(for: target).contains(found.sha256))
        } catch {
            fail(error)
            return nil
        }
    }

    /// Pins a certificate that was read, once the user has compared it. One that does not verify
    /// against the public CAs is refused, whatever the user says.
    @discardableResult
    public func trust(_ offer: PinOffer) -> Bool {
        do {
            guard offer.chainValid else { throw PinError.notVerifiable }
            try updateSettings { try $0.addPins([offer.sha256], for: offer.target) }
            return true
        } catch {
            fail(error)
            return false
        }
    }

    /// Pins what was pasted: one SHA-256 per line, as `tls-pin-get` prints them. All of them or none.
    @discardableResult
    public func addPins(pasted text: String, for target: PinTarget) -> Bool {
        do {
            let pins = try parsePins(text: text)
            try updateSettings { try $0.addPins(pins, for: target) }
            return true
        } catch {
            fail(error)
            return false
        }
    }

    public func removePin(_ pin: String, for target: PinTarget) {
        do {
            try updateSettings { $0.removePin(pin, for: target) }
        } catch {
            fail(error)
        }
    }

    /// Where the certificate of the attachments storage is read from.
    public func setAttachmentsHost(_ host: String) {
        do {
            try updateSettings { $0.attachmentsHost = host }
        } catch {
            fail(error)
        }
    }

    // MARK: Accounts and transactions

    /// The accounts with their balances, then the first page of transactions.
    public func refresh() async {
        await refreshAccounts()
        await refreshTransactions()
    }

    public func refreshAccounts() async {
        guard let service else { return }
        do {
            accounts = try await service.accounts()
        } catch {
            fail(error)
        }
    }

    public func select(_ account: BankAccount) async {
        guard account.id != selectedAccount?.id else { return }
        do {
            try updateSettings { $0.selectedAccountId = account.id }
        } catch {
            fail(error)
        }
        startOver()
        await refreshTransactions()
    }

    public func setFilter(_ newFilter: ListFilter) async {
        guard newFilter != filter else { return }
        filter = newFilter
        startOver()
        await refreshTransactions()
    }

    /// Another account or another filter: what is listed is not what was asked for anymore.
    private func startOver() {
        generation += 1
        transactions = []
        (nextPage, totalCount) = (nil, nil)
    }

    /// Loads the first page again. What is on screen stays until it is there.
    public func refreshTransactions() async {
        generation += 1
        await loadPage(1, replacing: true)
    }

    /// Loads the next page, if there is one and nothing is being loaded.
    public func loadMoreTransactions() async {
        guard let nextPage, !isLoadingTransactions else { return }
        await loadPage(nextPage, replacing: false)
    }

    /// For the row that appears on screen: the last one asks for the next page.
    public func loadMoreIfNeeded(after transaction: Transaction) async {
        if transaction.id == transactions.last?.id {
            await loadMoreTransactions()
        }
    }

    private func loadPage(_ page: UInt32, replacing: Bool) async {
        guard let service, let account = selectedAccount else { return }
        let asked = generation
        isLoadingTransactions = true
        defer {
            if asked == generation { isLoadingTransactions = false }
        }
        do {
            let fetched = try await service.transactions(
                accountId: account.id, filter: filter.transactionFilter, page: page)
            guard asked == generation else { return }
            if replacing {
                transactions = fetched.transactions
            } else {
                // A transaction that arrived meanwhile shifts the pages: one can come twice.
                let known = Set(transactions.map(\.id))
                transactions += fetched.transactions.filter { !known.contains($0.id) }
            }
            (nextPage, totalCount) = (fetched.nextPage, fetched.totalCount)
        } catch {
            if asked == generation { fail(error) }
        }
    }

    // MARK: Attachments

    /// Asks the API what a transaction has.
    public func loadAttachments(of transaction: Transaction) async {
        guard let service else { return }
        do {
            attachments[transaction.id] = try await service.attachments(transactionId: transaction.id)
        } catch {
            fail(error)
        }
    }

    /// How many attachments a transaction has: as last asked, or else as the list said.
    public func attachmentCount(of transaction: Transaction) -> Int {
        attachments[transaction.id]?.count ?? transaction.attachmentIds.count
    }

    /// Downloads an attachment (unless it is already here) and returns where it is, to preview or share it.
    public func localCopy(of attachment: Attachment, in transaction: Transaction) async -> URL? {
        guard let service else { return nil }
        let id = attachment.id
        downloads[id] = 0
        defer { downloads[id] = nil }
        let progress: @Sendable (Double) -> Void = { fraction in
            Task { @MainActor [weak self] in
                guard let self, self.downloads[id] != nil else { return }
                self.downloads[id] = fraction
            }
        }
        do {
            do {
                return URL(fileURLWithPath: try await service.download(
                    attachment, into: downloadDirectory, progress: progress).path)
            } catch BankError.Http {
                // Its URL only lasts a while: ask for a new one, once.
                let fresh = try await service.attachments(transactionId: transaction.id)
                attachments[transaction.id] = fresh
                guard let again = fresh.first(where: { $0.id == id }) else { return nil }
                return URL(fileURLWithPath: try await service.download(
                    again, into: downloadDirectory, progress: progress).path)
            }
        } catch {
            fail(error)
            return nil
        }
    }

    /// Removes the downloaded copies. Nothing is deleted at the bank.
    public func clearDownloads() {
        try? FileManager.default.removeItem(at: downloadDirectory)
    }

    /// Sends a receipt (PDF, JPEG or PNG) to a transaction. `cleanUp` removes a temporary copy afterwards.
    /// A file that the transaction already has is not sent: it waits in `pendingReceipt`, as does one for
    /// which that could not be checked. Returns whether the file was sent.
    @discardableResult
    public func attach(
        fileAt url: URL, to transaction: Transaction, force: Bool = false, cleanUp: Bool = false
    ) async -> Bool {
        var kept = false
        defer {
            if cleanUp && !kept { try? FileManager.default.removeItem(at: url) }
        }
        guard let service else { return false }
        let id = UUID()
        uploads.append(UploadInProgress(id: id, transactionId: transaction.id, fraction: 0))
        defer { uploads.removeAll { $0.id == id } }
        let wait = { (reason: PendingReceipt.Reason) in
            self.pendingReceipt = PendingReceipt(transaction: transaction, file: url, reason: reason, cleanUp: cleanUp)
            kept = true
        }
        do {
            let outcome = try await service.attach(
                fileAt: url, to: transaction.id, force: force, attachmentsDirectory: downloadDirectory
            ) { fraction in
                Task { @MainActor [weak self] in
                    guard let self, let index = self.uploads.firstIndex(where: { $0.id == id }) else { return }
                    self.uploads[index].fraction = fraction
                }
            }
            switch outcome {
            case .alreadyThere:
                wait(.alreadyThere)
                return false
            case .uploaded:
                await didAttach(to: transaction)
                return true
            }
        } catch BankError.NotChecked(let message) {
            wait(.notChecked(message))
            return false
        } catch {
            fail(error)
            return false
        }
    }

    private func didAttach(to transaction: Transaction) async {
        if let service, let fresh = try? await service.attachments(transactionId: transaction.id) {
            attachments[transaction.id] = fresh
        }
        // It has a receipt now: it does not belong in the list of the ones that miss one.
        if filter.missingReceiptOnly, let index = transactions.firstIndex(where: { $0.id == transaction.id }) {
            transactions.remove(at: index)
            totalCount = totalCount.map { $0 > 0 ? $0 - 1 : 0 }
        }
    }

    /// The upload progress of a transaction, if a receipt is on its way to it.
    public func uploadFraction(for transaction: Transaction) -> Double? {
        uploads.first { $0.transactionId == transaction.id }?.fraction
    }

    /// Sends the pending receipt without asking what the transaction has: a new attachment, whatever is there.
    @discardableResult
    public func sendPendingReceipt() async -> Bool {
        guard let pending = pendingReceipt else { return false }
        pendingReceipt = nil
        return await attach(fileAt: pending.file, to: pending.transaction, force: true, cleanUp: pending.cleanUp)
    }

    public func dropPendingReceipt() {
        if let pending = pendingReceipt, pending.cleanUp {
            try? FileManager.default.removeItem(at: pending.file)
        }
        pendingReceipt = nil
    }
}
