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
                        NavigationLink(value: name) {
                            EntryLabel(name: name, isLocked: model.locked.contains(name))
                        }
                    }
                } else {
                    OutlineGroup(model.tree, children: \.folderChildren) { node in
                        if node.isFolder {
                            FolderRow(node: node)
                        } else {
                            NavigationLink(value: node.path) {
                                EntryLabel(name: node.name, isLocked: node.isLocked)
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

/// An entry in the list. One this device cannot read is there by name, with a lock.
struct EntryLabel: View {
    let name: String
    let isLocked: Bool

    var body: some View {
        Label(name, systemImage: isLocked ? "lock.slash" : "key")
            .foregroundStyle(isLocked ? .secondary : .primary)
    }
}

/// A folder, and under its name who can read what is in it.
struct FolderRow: View {
    let node: StoreNode
    @Environment(AppModel.self) private var model
    @State private var access: Access?

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 2) {
                Text(node.name)
                if let access {
                    Text("Readable by: \(describe(access.readers))")
                        .font(.caption)
                        .foregroundStyle(access.readers.contains { $0.device == nil } ? .orange : .secondary)
                }
            }
        } icon: {
            Image(systemName: "folder")
        }
        .badge(node.entryCount)
        .task(id: model.entries) { access = await model.access(toFolder: node.path) }
    }
}

/// The sync button and, under it, where the device stands. Leads to the review when the owner has published something.
struct SyncRow: View {
    @Environment(AppModel.self) private var model
    @State private var reviewing = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button {
                Task {
                    await model.sync()
                    reviewing = model.pendingUpdate != nil
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
            if model.pendingUpdate != nil {
                Button("Look at the owner's update…") { reviewing = true }
                    .font(.callout)
            }
        }
        .sheet(isPresented: $reviewing) {
            NavigationStack { ReviewView() }
        }
    }
}
