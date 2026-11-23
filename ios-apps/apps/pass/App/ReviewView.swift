import PassKit
import SwiftUI

/// What one device published, before any of it is taken: what it does to the entries and to
/// who reads them, in the words of SYNC.md, the commits, and a choice for each thing both
/// sides did differently. Take it, or leave it for later.
struct ReviewView: View {
    let device: String
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var busy = false
    @State private var loaded = false
    @State private var confirmingDrop = false

    var body: some View {
        Form {
            if let update = model.review, update.device == device {
                summary(update)
                let names = conflicts(in: update)
                if !names.isEmpty {
                    Section {
                        ForEach(names, id: \.self) { name in
                            ConflictRow(name: name, device: device)
                        }
                    } header: {
                        Text("Done differently on both sides")
                    } footer: {
                        Text("Each is one or the other, whole: this device's side, or \(device)'s. A name ending in / is the readers of that folder.")
                    }
                }
                Section("What it does") {
                    ForEach(Array(update.lines.enumerated()), id: \.offset) { _, line in
                        ReviewLineRow(line: line)
                    }
                    if update.lines.isEmpty {
                        Text("Nothing to the entries or their readers: the same store, encrypted again.")
                            .foregroundStyle(.secondary)
                    }
                }
                if update.reencrypts > 0 {
                    Section {
                        Label(
                            "Taking it encrypts \(update.reencrypts) entries again, for the readers their folder then names.",
                            systemImage: "person.badge.key")
                    } footer: {
                        Text("That is how a device that was named a reader gets to read: by a device that already reads encrypting for it. Look at the readers above before you do.")
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
            } else if loaded {
                Text("Nothing to take from \(device).")
                Section {
                    Button("Drop this device's changes and stand where \(device) does", role: .destructive) {
                        confirmingDrop = true
                    }
                    .disabled(busy)
                }
            } else {
                ProgressView()
            }
        }
        .navigationTitle("From \(device)")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Later") {
                    model.closeReview()
                    dismiss()
                }
            }
        }
        .overlay {
            if busy { ProgressView() }
        }
        .task {
            await model.openReview(of: device)
            loaded = true
        }
        .confirmationDialog(
            "Everything this device has that \(device) has not published is given up here. Other devices that took it still have it.",
            isPresented: $confirmingDrop, titleVisibility: .visible
        ) {
            Button("Drop it", role: .destructive) {
                Task {
                    busy = true
                    defer { busy = false }
                    await model.drop(to: device)
                    dismiss()
                }
            }
        }
        // Locked: nothing decrypted stays on screen, and nothing can be taken.
        .onChange(of: model.isUnlocked) { _, unlocked in
            if !unlocked { dismiss() }
        }
    }

    private func summary(_ update: PendingUpdate) -> some View {
        Section {
            LabeledContent("Commits", value: "\(update.commitCount)")
            if let peer = model.peers.first(where: { $0.device == device }), let ago = model.publishedAgo(peer) {
                LabeledContent("Signed by \(device)", value: ago)
            }
            switch update.kind {
            case .adopt:
                Text("This device has no store yet: taking this makes \(device)'s store its own.").font(.callout)
            case .forward:
                Text("It continues what this device has: taking it moves the store forward.").font(.callout)
            case .merge:
                Text("This device has changes of its own: taking this merges the two. Nothing of yours is lost unless you choose so.")
                    .font(.callout)
            }
        } footer: {
            Text("The file was signed by \(device)'s key, is complete, and holds nothing but entries and their readers. Nothing of it is in the store yet.")
        }
    }

    private func decision(_ update: PendingUpdate) -> some View {
        Section {
            Button(describe(update.kind)) {
                Task {
                    busy = true
                    defer { busy = false }
                    if await model.take() { dismiss() }
                }
            }
            .disabled(busy || !model.undecided.isEmpty)
            if update.kind == .merge {
                Button("Drop this device's changes instead", role: .destructive) {
                    confirmingDrop = true
                }
                .disabled(busy)
            }
        } footer: {
            if !model.undecided.isEmpty {
                Text("Choose a side for: \(model.undecided.joined(separator: ", ")).")
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

/// One line of the review, as SYNC.md words it, with a mark for what deserves a second look.
struct ReviewLineRow: View {
    let line: ReviewLine

    var body: some View {
        Label {
            Text(reviewLineText(line: line)).font(.callout.monospaced())
        } icon: {
            Image(systemName: icon).foregroundStyle(tint)
        }
    }

    private var icon: String {
        switch line {
        case .added: return "plus.circle"
        case .changed: return "pencil.circle"
        case .moved: return "arrow.right.circle"
        case .removed: return "minus.circle"
        case .readers: return "person.2.circle"
        case .stale: return "lock.rotation"
        case .conflict: return "exclamationmark.triangle"
        }
    }

    private var tint: Color {
        switch line {
        case .readers, .stale: return .orange
        case .conflict, .removed: return .red
        default: return .secondary
        }
    }
}

/// One thing both sides did differently: both sides can be read, and one is chosen, whole.
struct ConflictRow: View {
    let name: String
    let device: String
    @Environment(AppModel.self) private var model
    @State private var versions: EntryVersions?
    @State private var loading = false

    private var isFolder: Bool { name.hasSuffix("/") }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(isFolder ? "The readers of \(name)" : name).font(.headline)
            Picker("Keep", selection: Binding(
                get: { model.choices[name] },
                set: { if let side = $0 { model.choose(side, for: name) } })
            ) {
                Text("Choose").tag(Side?.none)
                Text("Mine").tag(Side?.some(.mine))
                Text("\(device)'s").tag(Side?.some(.theirs))
            }
            .pickerStyle(.segmented)
            if let versions {
                VersionText(title: "Mine", version: versions.mine)
                VersionText(title: "\(device)'s", version: versions.theirs)
                VersionText(title: "Before", version: versions.base)
                Button("Hide") { self.versions = nil }.font(.caption)
            } else if !isFolder {
                Button(loading ? "Decrypting…" : "Show both") {
                    Task {
                        loading = true
                        defer { loading = false }
                        versions = await model.versions(of: name)
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
