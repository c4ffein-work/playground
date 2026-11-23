import BankKit
import SwiftUI

@main
struct BankApp: App {
    @State private var model = AppModel(
        store: .shared,
        downloadDirectory: URL.cachesDirectory.appending(path: "attachments"))

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
        }
    }
}

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if model.isConnected {
                TabView {
                    TransactionsView()
                        .tabItem { Label("Transactions", systemImage: "list.bullet.rectangle") }
                    AccountsView()
                        .tabItem { Label("Accounts", systemImage: "building.columns") }
                    NavigationStack { SettingsView() }
                        .tabItem { Label("Settings", systemImage: "gear") }
                        .badge(model.pinProblem == nil ? 0 : 1)
                }
            } else {
                NavigationStack { SettingsView() }
            }
        }
        .alert(
            "Bank",
            isPresented: Binding(
                get: { model.errorMessage != nil },
                set: { if !$0 { model.errorMessage = nil } })
        ) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(model.errorMessage ?? "")
        }
        // A receipt that was not sent, and why: the CLI's "add 'force' to send it again".
        .confirmationDialog(
            pendingTitle,
            isPresented: Binding(
                get: { model.pendingReceipt != nil },
                set: { if !$0 { model.dropPendingReceipt() } }),
            titleVisibility: .visible
        ) {
            Button("Send anyway") {
                Task { await model.sendPendingReceipt() }
            }
            Button("Cancel", role: .cancel) {
                model.dropPendingReceipt()
            }
        } message: {
            Text(pendingMessage)
        }
    }

    private var pendingTitle: String {
        switch model.pendingReceipt?.reason {
        case .notChecked: return "Could not check what this transaction already has"
        default: return "This transaction already has this file"
        }
    }

    private var pendingMessage: String {
        switch model.pendingReceipt?.reason {
        case .notChecked(let why): return "Nothing was sent. \(why)"
        default: return "Nothing was sent. Sending it anyway attaches a second copy."
        }
    }
}
