import BankKit
import SwiftUI

/// The bank accounts of the organization, with their balances. Tap one to list its transactions.
struct AccountsView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        NavigationStack {
            List(model.accounts) { account in
                Button {
                    Task { await model.select(account) }
                } label: {
                    AccountRow(account: account, selected: account.id == model.selectedAccount?.id)
                }
            }
            .overlay {
                if model.accounts.isEmpty {
                    ContentUnavailableView("No account", systemImage: "building.columns")
                }
            }
            .navigationTitle("Accounts")
            .refreshable { await model.refreshAccounts() }
            .task { await model.refreshAccounts() }
        }
    }
}

struct AccountRow: View {
    let account: BankAccount
    let selected: Bool

    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 4) {
                Text(account.displayName)
                    .font(.headline)
                    .foregroundStyle(.primary)
                if let iban = account.groupedIBAN {
                    Text(iban)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                }
                if let authorized = account.authorizedBalance {
                    Text("Available: \(authorized.formatted())")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer()
            VStack(alignment: .trailing, spacing: 4) {
                if let balance = account.balance {
                    Text(balance.formatted())
                        .font(.headline.monospacedDigit())
                        .foregroundStyle(.primary)
                }
                if selected {
                    Image(systemName: "checkmark")
                        .foregroundStyle(.tint)
                }
            }
        }
    }
}
