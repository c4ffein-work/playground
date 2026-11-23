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
                SyncSection()
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
                        ReadersView(folder: "")
                    } label: {
                        Image(systemName: "person.2")
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
    @State private var editingReaders = false

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
        .swipeActions {
            Button("Readers") { editingReaders = true }.tint(.orange)
        }
        .contextMenu {
            Button("Readers…", systemImage: "person.2") { editingReaders = true }
        }
        .sheet(isPresented: $editingReaders) {
            NavigationStack {
                ReadersView(folder: node.path)
                    .toolbar {
                        ToolbarItem(placement: .cancellationAction) {
                            Button("Done") { editingReaders = false }
                        }
                    }
            }
        }
    }
}

/// The sync button, where the device stands, and the other devices: what each has to take,
/// which leads to its review.
struct SyncSection: View {
    @Environment(AppModel.self) private var model
    @State private var reviewing: ReviewTarget?

    var body: some View {
        Section {
            Button {
                Task { await model.sync() }
            } label: {
                HStack {
                    Label("Sync", systemImage: "arrow.triangle.2.circlepath")
                    Spacer()
                    if model.isSyncing { ProgressView() }
                }
            }
            .disabled(model.isSyncing)
            ForEach(model.peers, id: \.device) { peer in
                Button {
                    reviewing = ReviewTarget(device: peer.device)
                } label: {
                    LabeledContent {
                        Text(model.standing(of: peer))
                            .foregroundStyle(peer.toTake > 0 ? Color.accentColor : .secondary)
                    } label: {
                        Text(peer.device)
                        if let ago = model.publishedAgo(peer) {
                            Text(ago)
                        }
                    }
                }
                .disabled(!peer.received)
            }
        } header: {
            Text("Devices")
        } footer: {
            VStack(alignment: .leading, spacing: 4) {
                if let message = model.syncMessage {
                    Text(message)
                }
                if model.syncState?.unpublished == true {
                    Text("This device's changes are not published yet.")
                }
                if !model.stale.isEmpty {
                    Text("\(model.stale.count) entries are not encrypted for the readers their folder names.")
                }
            }
        }
        .sheet(item: $reviewing) { target in
            NavigationStack { ReviewView(device: target.device) }
        }
    }
}

/// Whose update is being looked at.
struct ReviewTarget: Identifiable {
    let device: String
    var id: String { device }
}
