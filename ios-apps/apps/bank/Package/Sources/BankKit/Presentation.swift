import Foundation

// How what the API gives is shown. Nothing here computes with money: an amount is the integer of cents
// that the API sent, until the very moment it becomes text.

/// An amount: cents as the API gives them, and the currency code that came with them.
public struct Money: Equatable, Hashable, Sendable {
    /// Negative for money that left the account.
    public let cents: Int64
    /// ISO 4217, as the API says: "EUR".
    public let currency: String

    public init(cents: Int64, currency: String) {
        self.cents = cents
        self.currency = currency
    }

    /// The same amount as an exact decimal number of units: cents × 10⁻². No division, no binary float.
    /// Two decimals whatever the currency: it is what "cents" says, and what the CLI assumes.
    public var decimal: Decimal {
        Decimal(sign: cents < 0 ? .minus : .plus, exponent: -2, significand: Decimal(cents.magnitude))
    }

    /// For people, in their locale: "-1 234,56 €", "$99.99".
    public func formatted(locale: Locale = .current) -> String {
        decimal.formatted(.currency(code: currency).locale(locale))
    }
}

extension BankAccount: Identifiable {}

extension BankAccount {
    public var displayName: String {
        if let name, !name.isEmpty { return name }
        if let iban, !iban.isEmpty { return iban }
        return "Account …\(id.suffix(6))"
    }

    public var balance: Money? {
        balanceCents.map { Money(cents: $0, currency: currency ?? "EUR") }
    }

    /// What can still be spent: the balance, less what is authorized but not settled. Only when it differs.
    public var authorizedBalance: Money? {
        guard let cents = authorizedBalanceCents, cents != balanceCents else { return nil }
        return Money(cents: cents, currency: currency ?? "EUR")
    }

    /// `FR76 1695 8000 …`
    public var groupedIBAN: String? {
        iban.map(groupedFingerprint)
    }
}

extension Transaction: Identifiable {}

extension Transaction {
    public var isDebit: Bool { side == "debit" }

    /// In the currency of the account, negative for a debit. The API gives a positive amount and a side.
    public var amount: Money {
        Money(cents: isDebit ? -amountCents : amountCents, currency: currency)
    }

    /// What was paid abroad, in the currency it was paid in. `nil` for a domestic transaction.
    /// The CLI: `use-original-currency`.
    public var localAmount: Money? {
        guard let localCurrency, localCurrency != currency else { return nil }
        return Money(cents: isDebit ? -localAmountCents : localAmountCents, currency: localCurrency)
    }

    /// The end of the readable id: what the CLI shows, and takes, to name a transaction.
    public var shortId: String { String(transactionId.suffix(6)) }

    public var title: String {
        if !label.isEmpty { return label }
        if let name = cleanCounterpartyName, !name.isEmpty { return name }
        return shortId
    }

    public var emittedDate: Date? { parseTimestamp(emittedAt) }
    public var settledDate: Date? { settledAt.flatMap(parseTimestamp) }

    /// As the list of transactions said. `AppModel.attachmentCount(of:)` knows better after an upload.
    public var hasAttachments: Bool { !attachmentIds.isEmpty }

    /// An SF Symbol for the kind of operation.
    public var symbolName: String {
        switch operationType ?? "" {
        case "card": return "creditcard"
        case "transfer": return "arrow.left.arrow.right"
        case "income": return "arrow.down.circle"
        case "direct_debit": return "repeat"
        case "cheque", "check": return "signature"
        default: return isDebit ? "arrow.up.right" : "arrow.down.left"
        }
    }
}

/// `2024-06-15T10:30:00.000Z`, with or without the milliseconds.
func parseTimestamp(_ text: String) -> Date? {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let date = formatter.date(from: text) {
        return date
    }
    formatter.formatOptions = [.withInternetDateTime]
    return formatter.date(from: text)
}

extension Attachment: Identifiable {}

extension Attachment {
    /// An SF Symbol for the kind of file, from what the API says it is.
    public var symbolName: String {
        guard let type = fileContentType else { return "paperclip" }
        if type == "application/pdf" { return "doc.richtext" }
        return type.hasPrefix("image/") ? "photo" : "paperclip"
    }

    public var createdDate: Date? { createdAt.flatMap(parseTimestamp) }
}

// MARK: Which transactions

/// A calendar day, with no time and no zone.
public struct Day: Equatable, Hashable, Comparable, Sendable, CustomStringConvertible {
    public let year: Int
    public let month: Int
    public let day: Int

    public init(year: Int, month: Int, day: Int) {
        (self.year, self.month, self.day) = (year, month, day)
    }

    /// The day that `date` is in, for `calendar` (the one of the user: what a date picker shows).
    public init(_ date: Date, calendar: Calendar = .current) {
        let parts = calendar.dateComponents([.year, .month, .day], from: date)
        self.init(year: parts.year ?? 1970, month: parts.month ?? 1, day: parts.day ?? 1)
    }

    public static func < (a: Day, b: Day) -> Bool {
        (a.year, a.month, a.day) < (b.year, b.month, b.day)
    }

    /// `2024-01-31`
    public var description: String {
        let pad = { (value: Int, width: Int) in
            String(repeating: "0", count: max(0, width - String(value).count)) + String(value)
        }
        return "\(pad(year, 4))-\(pad(month, 2))-\(pad(day, 2))"
    }
}

/// The dates of the transactions to list. The CLI only knows years (`date=2024`, `date>=2023`...);
/// days are the same thing, finer. As in the CLI, a day is a day of UTC: the API is asked for
/// `…T00:00:00Z` to `…T23:59:59Z`, so around midnight a transaction can fall on the other side.
public enum Period: Equatable, Hashable, Sendable {
    case all
    /// The CLI's `date=2024`.
    case year(Int)
    /// Both ends included, either one optional: the CLI's `date>=` and `date<=`.
    case days(from: Day?, to: Day?)

    public var from: Day? {
        switch self {
        case .all: return nil
        case .year(let year): return Day(year: year, month: 1, day: 1)
        case .days(let from, _): return from
        }
    }

    public var to: Day? {
        switch self {
        case .all: return nil
        case .year(let year): return Day(year: year, month: 12, day: 31)
        case .days(_, let to): return to
        }
    }

    /// A range that ends before it starts lists nothing: the core refuses it, the views do not offer it.
    public var isValid: Bool {
        guard let from, let to else { return true }
        return from <= to
    }

    /// The year it is for the user, or one before (`yearsAgo: 1`: what is being closed in January).
    public static func calendarYear(yearsAgo: Int = 0, now: Date = Date(), calendar: Calendar = .current) -> Period {
        .year(calendar.component(.year, from: now) - yearsAgo)
    }

    /// For the button that opens the choice of dates.
    public var title: String {
        switch self {
        case .all: return "All dates"
        case .year(let year): return String(year)
        case .days(let from?, let to?): return from == to ? "\(from)" : "\(from) – \(to)"
        case .days(let from?, nil): return "Since \(from)"
        case .days(nil, let to?): return "Until \(to)"
        case .days(nil, nil): return "All dates"
        }
    }
}

/// What the list of transactions is filtered by.
public struct ListFilter: Equatable, Hashable, Sendable {
    /// Only the transactions with no attachment: the ones that still need a receipt. The CLI's `no-invoice`.
    public var missingReceiptOnly: Bool
    public var period: Period

    public init(missingReceiptOnly: Bool = false, period: Period = .all) {
        self.missingReceiptOnly = missingReceiptOnly
        self.period = period
    }

    /// What the core sends to the API.
    public var transactionFilter: TransactionFilter {
        TransactionFilter(
            withAttachments: missingReceiptOnly ? false : nil,
            emittedAtFrom: period.from.map { "\($0)T00:00:00Z" },
            emittedAtTo: period.to.map { "\($0)T23:59:59Z" })
    }
}
