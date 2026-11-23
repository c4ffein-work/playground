import PassKit
import SwiftUI

/// What the owner published, before any of it is taken: the commits, the entries, who can
/// read what, and for the entries both sides changed, the choice. Accept, or leave it for later.
struct ReviewView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var busy = false
    @State private var confirmingDrop = false

    var body: some View {
        Form {
            if let update = model.pendingUpdate {
                summary(update)
                if !update.recipientChanges.isEmpty {
                    Section {
                        ForEach(update.recipientChanges, id: \.path) { change in
                            RecipientChangeRow(change: change)
                        }
                    } header: {
                        Text("Who can read")
                    } footer: {
                        Text("Only the owner changes this. Check it is what you did on the computer: everything written from now on is readable by whoever is listed.")
                    }
                }
                if !update.entries.isEmpty {
                    Section("Entries") {
                        ForEach(update.entries, id: \.name) { entry in
                            EntryChangeRow(entry: entry)
                        }
                    }
                }
                if !update.otherFiles.isEmpty {
                    Section {
                        ForEach(update.otherFiles, id: \.self) { path in
                            Text(path).font(.caption.monospaced())
                        }
                    } header: {
                        Text("Other files")
                    } footer: {
                        Text("Not entries: taken from the owner as they are.")
                    }
                }
                Section("Commits") {
                    ForEach(update.commits, id: \.id) { commit in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(commit.subject)
                            Text("\(commit.author) · \(signatureText(commit.signature))")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    if update.commitCount > update.commits.count {
                        Text("…and \(Int(update.commitCount) - update.commits.count) older ones")
                            .foregroundStyle(.secondary)
                    }
                }
                decision(update)
            } else {
                Text("Nothing from the owner is waiting.")
            }
        }
        .navigationTitle("The owner's update")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Later") { dismiss() }
            }
        }
        .overlay {
            if busy { ProgressView() }
        }
        .confirmationDialog(
            "The changes made on this device that the owner has not taken will be lost.",
            isPresented: $confirmingDrop, titleVisibility: .visible
        ) {
            Button("Drop them", role: .destructive) {
                Task {
                    busy = true
                    defer { busy = false }
                    await model.discardOutstanding()
                }
            }
        }
        // Locked: nothing decrypted stays on screen, and nothing can be accepted.
        .onChange(of: model.isUnlocked) { _, unlocked in
            if !unlocked { dismiss() }
        }
    }

    private func summary(_ update: PendingUpdate) -> some View {
        Section {
            LabeledContent("Commits", value: "\(update.commitCount)")
            if let ago = model.ownerPublishedAgo {
                LabeledContent("Signed by the owner", value: ago)
            }
            if update.outstanding > 0 {
                Text("This device has \(update.outstanding) change(s) of its own: accepting merges the two, and nothing of yours is lost unless you choose so below.")
                    .font(.callout)
            }
        } footer: {
            Text("The file was signed by the owner's key and is complete. Nothing of it is in the store yet.")
        }
    }

    private func decision(_ update: PendingUpdate) -> some View {
        Section {
            Button(update.outstanding > 0 ? "Accept and merge" : "Accept") {
                Task {
                    busy = true
                    defer { busy = false }
                    if await model.acceptUpdate() { dismiss() }
                }
            }
            .disabled(busy || !model.undecided.isEmpty)
            if update.outstanding > 0 {
                Button("Drop this device's changes instead", role: .destructive) {
                    confirmingDrop = true
                }
                .disabled(busy)
            }
        } footer: {
            if !model.undecided.isEmpty {
                Text("Choose a version for: \(model.undecided.joined(separator: ", ")).")
            }
        }
    }
}

func signatureText(_ status: SignatureStatus) -> String {
    switch status {
    case .unsigned: return "not signed"
    case .valid(let device): return "signed by \(device)"
    case .invalid: return "signed by an unknown key"
    }
}

/// A `.gpg-id` before and after, by device name.
struct RecipientChangeRow: View {
    let change: RecipientChange

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(change.path).font(.headline.monospaced())
            LabeledContent("Before", value: change.before.isEmpty ? "(new)" : describe(change.before))
            LabeledContent("After", value: change.after.isEmpty ? "(removed)" : describe(change.after))
            ForEach(change.after.filter { added in !change.before.contains { $0.id == added.id } }, id: \.id) { reader in
                Label(
                    reader.device.map { "\($0) can now read" } ?? "A key that is no device's can now read: \(reader.id)",
                    systemImage: "plus.circle.fill")
                    .foregroundStyle(reader.device == nil ? .red : .orange)
            }
        }
        .font(.callout)
    }
}

/// One entry of the update: what happens to it, who can read it now, and the choice when both sides changed it.
struct EntryChangeRow: View {
    let entry: EntryChange
    @Environment(AppModel.self) private var model
    @State private var versions: EntryVersions?
    @State private var loading = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(entry.name)
                Spacer()
                Text(describe(entry.kind)).font(.caption).foregroundStyle(.secondary)
            }
            if !entry.readersAfter.isEmpty {
                Text(readersLine)
                    .font(.caption)
                    .foregroundStyle(entry.readersBefore.isEmpty ? .secondary : .orange)
            }
            if entry.conflict {
                Text("Changed on this device too.").font(.caption).foregroundStyle(.red)
                Picker("Keep", selection: Binding(
                    get: { model.choices[entry.name] },
                    set: { if let side = $0 { model.choose(side, for: entry.name) } })
                ) {
                    Text("Choose").tag(Side?.none)
                    Text("Mine").tag(Side?.some(.mine))
                    Text("The owner's").tag(Side?.some(.theirs))
                }
                .pickerStyle(.segmented)
            }
            if let versions {
                if entry.conflict {
                    VersionText(title: "Mine", version: versions.mine)
                }
                VersionText(title: "Before", version: versions.before)
                VersionText(title: "The owner's", version: versions.theirs)
                Button("Hide") { self.versions = nil }.font(.caption)
            } else {
                Button(loading ? "Decrypting…" : "Show its content") {
                    Task {
                        loading = true
                        defer { loading = false }
                        versions = await model.versions(of: entry.name)
                    }
                }
                .font(.caption)
                .disabled(loading)
            }
        }
        .onChange(of: model.isUnlocked) { _, unlocked in
            if !unlocked { versions = nil }
        }
    }

    private var readersLine: String {
        let after = entry.readersAfter.joined(separator: ", ")
        guard !entry.readersBefore.isEmpty else { return "Readable by: \(after)" }
        return "Readable by: \(entry.readersBefore.joined(separator: ", ")) → \(after)"
    }
}

/// One version of an entry: its text, or why there is none to show.
struct VersionText: View {
    let title: String
    let version: EntryVersion

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            switch version {
            case .absent:
                Text("(no such entry)").foregroundStyle(.secondary)
            case .unreadable:
                Label("Not readable by this device", systemImage: "lock.slash").foregroundStyle(.secondary)
            case .content(let text):
                Text(text).font(.callout.monospaced()).textSelection(.enabled)
            }
        }
    }
}
