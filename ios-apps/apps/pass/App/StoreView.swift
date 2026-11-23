import PassKit
import SwiftUI

/// The store, unlocked: folders and entries, or what the search finds.
struct StoreView: View {
    @Environment(AppModel.self) private var model
    @State private var adding = false

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            List {
                if let results = model.searchResults {
                    ForEach(results, id: \.self) { name in
                        NavigationLink(name, value: name)
                    }
                } else {
                    OutlineGroup(model.tree, children: \.folderChildren) { node in
                        if node.isFolder {
                            Label(node.name, systemImage: "folder")
                                .badge(node.entryCount)
                        } else {
                            NavigationLink(value: node.path) {
                                Label(node.name, systemImage: "key")
                            }
                        }
                    }
                }
                Section {
                    SyncRow()
                }
            }
            .searchable(text: $model.searchText, prompt: "Search the store")
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            .overlay {
                if model.entries.isEmpty {
                    ContentUnavailableView(
                        "No entries", systemImage: "key", description: Text("Add one with +, or sync."))
                } else if model.searchResults?.isEmpty == true {
                    ContentUnavailableView.search(text: model.searchText)
                }
            }
            .navigationTitle("Passwords")
            .navigationDestination(for: String.self) { name in
                EntryView(name: name)
            }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button {
                        model.lock()
                    } label: {
                        Image(systemName: "lock")
                    }
                }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button {
                        adding = true
                    } label: {
                        Image(systemName: "plus")
                    }
                    NavigationLink {
                        SettingsView()
                    } label: {
                        Image(systemName: "gear")
                    }
                }
            }
            .sheet(isPresented: $adding) {
                NavigationStack { EditEntryView(existing: nil) }
            }
            .refreshable { await model.sync() }
            .task { await model.refresh() }
            // Scrolling and tapping around count as use.
            .simultaneousGesture(TapGesture().onEnded { model.userDidInteract() })
        }
    }
}

extension StoreNode {
    /// For `OutlineGroup`: `nil` is what makes a row a leaf.
    var folderChildren: [StoreNode]? { isFolder ? children : nil }
}

/// The sync button and, under it, where the device stands. Opens the details when something needs a decision.
struct SyncRow: View {
    @Environment(AppModel.self) private var model
    @State private var showingDetails = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                Task {
                    await model.sync()
                    showingDetails = model.pendingUpdate != nil || model.divergence != nil
                }
            } label: {
                HStack {
                    Label("Sync", systemImage: "arrow.triangle.2.circlepath")
                    Spacer()
                    if model.isSyncing { ProgressView() }
                }
            }
            .disabled(model.isSyncing)
            if let message = model.syncMessage {
                Text(message).font(.caption)
            }
            if let state = model.syncState {
                Text(describe(state)).font(.caption).foregroundStyle(.secondary)
            }
            if model.pendingUpdate != nil || model.divergence != nil {
                Button("Needs a decision…") { showingDetails = true }
                    .font(.caption)
            }
        }
        .sheet(isPresented: $showingDetails) {
            NavigationStack { SyncDecisionView() }
        }
    }
}

/// The two things a sync never does by itself: take new recipients, and choose between
/// this device's changes and the main branch.
struct SyncDecisionView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var confirmingDiscard = false

    var body: some View {
        Form {
            if let update = model.pendingUpdate {
                Section {
                    ForEach(update.recipientChanges, id: \.path) { change in
                        RecipientChangeRow(change: change)
                    }
                    LabeledContent("Last commit", value: signatureText(update.signature))
                    LabeledContent("Changes", value: "\(update.commits) commit(s), \(update.changed.count) entries")
                    Button("These are the right recipients: take the update") {
                        Task {
                            await model.acceptPendingUpdate()
                            dismiss()
                        }
                    }
                } header: {
                    Text("New recipients")
                } footer: {
                    Text("Everything written from now on will be readable by every key listed. Whoever can write to the relay can propose this: accept it only if you made the change yourself.")
                }
            }
            if let divergence = model.divergence {
                Section {
                    Text("This device has \(divergence.unmerged) change(s) that are not in the main branch, and the main branch has \(divergence.behind) that are not here.")
                    Text("On the computer, merge this device's branch and push: the next sync here then takes everything.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                    Button("Discard this device's changes", role: .destructive) {
                        confirmingDiscard = true
                    }
                } header: {
                    Text("Waiting for a merge")
                }
            }
            if model.pendingUpdate == nil && model.divergence == nil {
                Text("Nothing to decide.")
            }
        }
        .navigationTitle("Sync")
        .toolbar {
            Button("Close") { dismiss() }
        }
        .confirmationDialog(
            "The changes made on this device and not merged yet will be lost.",
            isPresented: $confirmingDiscard, titleVisibility: .visible
        ) {
            Button("Discard them", role: .destructive) {
                Task {
                    await model.discardLocalChanges()
                    dismiss()
                }
            }
        }
    }
}

func signatureText(_ status: SignatureStatus) -> String {
    switch status {
    case .unsigned: return "not signed"
    case .valid(let fingerprint): return "signed by …\(fingerprint.suffix(8))"
    case .invalid: return "signed by an unknown key"
    }
}

struct RecipientChangeRow: View {
    let change: RecipientChange

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(change.path).font(.headline)
            ForEach(change.after.filter { !change.before.contains($0) }, id: \.self) { id in
                Label(id, systemImage: "plus.circle.fill").foregroundStyle(.red)
            }
            ForEach(change.before.filter { !change.after.contains($0) }, id: \.self) { id in
                Label(id, systemImage: "minus.circle").foregroundStyle(.secondary)
            }
            Text(".gpg-id.sig: \(signatureText(change.signature))")
                .foregroundStyle(.secondary)
        }
        .font(.caption.monospaced())
    }
}
