import BankKit
import SwiftUI

/// The transactions of the selected account, newest first, a page at a time.
struct TransactionsView: View {
    @Environment(AppModel.self) private var model

    @State private var choosingDates = false

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Toggle("Missing a receipt", isOn: missingReceiptOnly)
                    Button {
                        choosingDates = true
                    } label: {
                        LabeledContent("Dates", value: model.filter.period.title)
                    }
                }
                Section {
                    ForEach(model.transactions) { transaction in
                        NavigationLink(value: transaction) {
                            TransactionRow(
                                transaction: transaction,
                                attachments: model.attachmentCount(of: transaction),
                                uploading: model.uploadFraction(for: transaction))
                        }
                        .onAppear {
                            Task { await model.loadMoreIfNeeded(after: transaction) }
                        }
                    }
                    if model.isLoadingTransactions {
                        HStack {
                            Spacer()
                            ProgressView()
                            Spacer()
                        }
                    }
                } header: {
                    if let total = model.totalCount {
                        Text("\(total) transaction(s)")
                    }
                }
            }
            .overlay {
                if model.transactions.isEmpty && !model.isLoadingTransactions {
                    ContentUnavailableView(
                        model.filter.missingReceiptOnly ? "Nothing misses a receipt" : "No transaction",
                        systemImage: model.filter.missingReceiptOnly ? "checkmark.seal" : "tray")
                }
            }
            .navigationTitle(model.selectedAccount?.displayName ?? "Transactions")
            .navigationDestination(for: Transaction.self) { transaction in
                TransactionDetailView(transaction: transaction)
            }
            .refreshable { await model.refresh() }
            .task {
                if model.accounts.isEmpty {
                    await model.refresh()
                }
            }
            .sheet(isPresented: $choosingDates) {
                PeriodSheet(period: model.filter.period) { period in
                    var filter = model.filter
                    filter.period = period
                    Task { await model.setFilter(filter) }
                }
            }
        }
    }

    private var missingReceiptOnly: Binding<Bool> {
        Binding(
            get: { model.filter.missingReceiptOnly },
            set: { only in
                var filter = model.filter
                filter.missingReceiptOnly = only
                Task { await model.setFilter(filter) }
            })
    }
}

struct TransactionRow: View {
    let transaction: Transaction
    let attachments: Int
    /// Set while a receipt is on its way.
    let uploading: Double?

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: transaction.symbolName)
                .foregroundStyle(.tint)
                .frame(width: 28)
            VStack(alignment: .leading, spacing: 2) {
                Text(transaction.title)
                    .lineLimit(1)
                HStack(spacing: 6) {
                    if let date = transaction.emittedDate {
                        Text(date.formatted(date: .abbreviated, time: .omitted))
                    }
                    if attachments > 0 {
                        Label("\(attachments)", systemImage: "paperclip")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                if let uploading {
                    ProgressView(value: uploading)
                }
            }
            Spacer()
            VStack(alignment: .trailing, spacing: 2) {
                Text(transaction.amount.formatted())
                    .font(.body.monospacedDigit())
                    .foregroundStyle(transaction.isDebit ? Color.primary : Color.green)
                if let local = transaction.localAmount {
                    Text(local.formatted())
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
            }
        }
    }
}

/// The dates of the transactions to list: everything, a year, or days.
struct PeriodSheet: View {
    private enum Choice: Hashable {
        case all, thisYear, lastYear, custom
    }

    let done: (Period) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var choice: Choice
    @State private var hasFrom: Bool
    @State private var hasTo: Bool
    @State private var from: Date
    @State private var to: Date

    init(period: Period, done: @escaping (Period) -> Void) {
        self.done = done
        let choice: Choice
        if period == .all {
            choice = .all
        } else if period == Period.calendarYear() {
            choice = .thisYear
        } else if period == Period.calendarYear(yearsAgo: 1) {
            choice = .lastYear
        } else {
            choice = .custom
        }
        _choice = State(initialValue: choice)
        _hasFrom = State(initialValue: period.from != nil)
        _hasTo = State(initialValue: period.to != nil)
        _from = State(initialValue: Self.date(of: period.from))
        _to = State(initialValue: Self.date(of: period.to))
    }

    /// Noon of that day for the user: what a date picker shows as that day.
    private static func date(of day: Day?) -> Date {
        guard let day else { return .now }
        let parts = DateComponents(year: day.year, month: day.month, day: day.day, hour: 12)
        return Calendar.current.date(from: parts) ?? .now
    }

    private var period: Period {
        switch choice {
        case .all: return .all
        case .thisYear: return .calendarYear()
        case .lastYear: return .calendarYear(yearsAgo: 1)
        case .custom: return .days(from: hasFrom ? Day(from) : nil, to: hasTo ? Day(to) : nil)
        }
    }

    var body: some View {
        NavigationStack {
            Form {
                Picker("Dates", selection: $choice) {
                    Text("All dates").tag(Choice.all)
                    Text("This year").tag(Choice.thisYear)
                    Text("Last year").tag(Choice.lastYear)
                    Text("Choose").tag(Choice.custom)
                }
                .pickerStyle(.inline)
                if choice == .custom {
                    Section {
                        Toggle("From", isOn: $hasFrom)
                        if hasFrom {
                            DatePicker("From", selection: $from, displayedComponents: .date)
                        }
                        Toggle("To", isOn: $hasTo)
                        if hasTo {
                            DatePicker("To", selection: $to, displayedComponents: .date)
                        }
                    } footer: {
                        Text(period.isValid
                            ? "Both days included. Days are days of UTC, as for the bank CLI."
                            : "The first day is after the last one.")
                    }
                }
            }
            .navigationTitle("Dates")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        done(period)
                        dismiss()
                    }
                    .disabled(!period.isValid)
                }
            }
        }
    }
}
