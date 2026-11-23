import PassKit
import SwiftUI

/// One entry, decrypted for as long as this view is on screen and the store is unlocked.
struct EntryView: View {
    let name: String

    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var entry: Entry?
    /// Who can read it. Known without the key, and before anything is decrypted.
    @State private var access: Access?
    @State private var revealed = false
    @State private var copied: String?
    @State private var editing = false
    @State private var renaming = false
    @State private var newName = ""
    @State private var confirmingDelete = false

    var body: some View {
        List {
            if let entry {
                Section("Password") {
                    CopyRow(
                        label: revealed ? entry.password : String(repeating: "•", count: 12),
                        secret: entry.password, what: "Password", copied: $copied)
                        .font(.body.monospaced())
                        .swipeActions {
                            Button(revealed ? "Hide" : "Show") { revealed.toggle() }
                        }
                    Toggle("Show", isOn: $revealed)
                }
                if !entry.otpUris.isEmpty {
                    Section("One-time code") {
                        ForEach(entry.otpUris, id: \.self) { uri in
                            OneTimeCodeRow(uri: uri, copied: $copied)
                        }
                    }
                }
                if !entry.fields.isEmpty {
                    Section("Fields") {
                        ForEach(entry.fields, id: \.self) { field in
                            LabeledContent(field.name) {
                                CopyRow(label: field.value, secret: field.value, what: field.name, copied: $copied)
                            }
                        }
                    }
                }
                if !entry.extra.isEmpty {
                    Section("Everything after the first line") {
                        Text(entry.extra)
                            .font(.callout.monospaced())
                            .textSelection(.enabled)
                    }
                }
            } else if access?.readable == false {
                Section {
                    Label("Not readable by this device", systemImage: "lock.slash")
                } footer: {
                    Text("It is encrypted to other keys than this device's. It can still be renamed or deleted here: the other devices see that before taking it.")
                }
            } else {
                ProgressView()
            }
            if let access {
                AccessSection(access: access)
            }
        }
        .navigationTitle(name.split(separator: "/").last.map(String.init) ?? name)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            Menu {
                if access?.readable != false {
                    Button {
                        editing = true
                    } label: {
                        Label("Edit", systemImage: "pencil")
                    }
                }
                Button {
                    newName = name
                    renaming = true
                } label: {
                    Label("Rename or move", systemImage: "folder")
                }
                Button(role: .destructive) {
                    confirmingDelete = true
                } label: {
                    Label("Delete", systemImage: "trash")
                }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
        }
        .safeAreaInset(edge: .bottom) {
            if let copied {
                Text("\(copied) copied: on this device only, for \(Int(model.settings.clipboardExpiry)) s")
                    .font(.caption)
                    .padding(8)
                    .background(.thinMaterial, in: Capsule())
            }
        }
        .sheet(isPresented: $editing, onDismiss: { Task { await load() } }) {
            NavigationStack { EditEntryView(existing: name) }
        }
        .alert("Rename or move", isPresented: $renaming) {
            TextField("Folder/name", text: $newName)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            Button("Move") {
                Task {
                    if await model.rename(name, to: newName) { dismiss() }
                }
            }
            Button("Cancel", role: .cancel) {}
        }
        .confirmationDialog("Delete \(name)?", isPresented: $confirmingDelete, titleVisibility: .visible) {
            Button("Delete", role: .destructive) {
                Task {
                    if await model.delete(name) { dismiss() }
                }
            }
        }
        .task { await load() }
        // Locked (by the timeout, the lock button, or leaving the app): nothing decrypted stays on screen.
        .onChange(of: model.isUnlocked) { _, unlocked in
            if !unlocked {
                entry = nil
                dismiss()
            }
        }
    }

    private func load() async {
        access = await model.access(toEntry: name)
        // What is not for this device is not decrypted, and is shown as such rather than as an error.
        guard access?.readable != false else { return }
        entry = await model.show(name)
        if entry == nil { dismiss() }
    }
}

/// "Readable by: pc, phone", and what is wrong with that, if anything.
struct AccessSection: View {
    let access: Access

    var body: some View {
        Section {
            LabeledContent("Readable by", value: describe(access.readers))
            ForEach(warnings(for: access), id: \.self) { warning in
                Label(warning, systemImage: "exclamationmark.triangle")
                    .font(.callout)
                    .foregroundStyle(.orange)
            }
        } footer: {
            Text("From \(access.gpgId.isEmpty ? "no .gpg-id" : access.gpgId): who can read is set on the computer, folder by folder.")
        }
    }
}

/// A value that is copied when tapped.
struct CopyRow: View {
    /// What is shown, which may be dots.
    let label: String
    /// What is copied.
    let secret: String
    /// What to say was copied: "Password".
    let what: String
    @Binding var copied: String?
    @Environment(AppModel.self) private var model

    var body: some View {
        Button {
            model.copy(secret)
            copied = what
        } label: {
            HStack {
                Text(label).lineLimit(1).foregroundStyle(.primary)
                Spacer()
                Image(systemName: "doc.on.doc").foregroundStyle(.secondary)
            }
        }
    }
}

/// The current code of an `otpauth://` line, and the time it has left.
struct OneTimeCodeRow: View {
    let uri: String
    @Binding var copied: String?
    @Environment(AppModel.self) private var model

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { _ in
            if let code = model.oneTimeCode(for: uri) {
                Button {
                    model.copy(code.code)
                    copied = "Code"
                } label: {
                    HStack {
                        Text(code.code).font(.title3.monospaced()).foregroundStyle(.primary)
                        Spacer()
                        Text("\(code.remaining) s").font(.caption).foregroundStyle(.secondary)
                        ProgressView(value: Double(code.remaining), total: Double(code.period))
                            .frame(width: 40)
                    }
                }
            } else {
                Text("Not a usable otpauth:// line").foregroundStyle(.secondary)
            }
        }
    }
}

/// A new entry, or the whole text of an existing one: the password on the first line, as in `pass edit`.
struct EditEntryView: View {
    /// The entry to edit, `nil` to add one.
    let existing: String?

    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var content = ""
    @State private var length = 25.0
    @State private var symbols = true
    @State private var busy = false

    var body: some View {
        Form {
            if existing == nil {
                Section {
                    TextField("Folder/name", text: $name)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } footer: {
                    Text("As in pass: Email/work puts the entry work in the folder Email.")
                }
            }
            Section {
                TextEditor(text: $content)
                    .font(.body.monospaced())
                    .frame(minHeight: 160)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
            } header: {
                Text("Content")
            } footer: {
                Text("The first line is the password. Then anything: `login: alice`, a URL, an otpauth:// line.")
            }
            Section("Generate the password") {
                Stepper("\(Int(length)) characters", value: $length, in: 8...128)
                Toggle("Symbols", isOn: $symbols)
                Button(existing == nil ? "Generate and save" : "Replace the first line and save") {
                    Task { await generate() }
                }
                .disabled(busy || (existing == nil && name.isEmpty))
            }
        }
        .navigationTitle(existing ?? "New entry")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Save") {
                    Task { await save() }
                }
                .disabled(busy || content.isEmpty || (existing == nil && name.isEmpty))
            }
        }
        .task {
            if let existing {
                content = await model.content(of: existing) ?? ""
            }
        }
        .onChange(of: model.isUnlocked) { _, unlocked in
            if !unlocked {
                content = ""
                dismiss()
            }
        }
    }

    private func save() async {
        busy = true
        defer { busy = false }
        let done: Bool
        if let existing {
            done = await model.save(existing, content: content)
        } else {
            done = await model.add(name, content: content)
        }
        if done { dismiss() }
    }

    private func generate() async {
        busy = true
        defer { busy = false }
        let password: String?
        if let existing {
            password = await model.regenerate(existing, length: Int(length), symbols: symbols)
        } else {
            password = await model.generate(name, length: Int(length), symbols: symbols)
        }
        if let password {
            model.copy(password)
            dismiss()
        }
    }
}
