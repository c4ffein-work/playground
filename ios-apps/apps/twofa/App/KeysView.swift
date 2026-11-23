import SwiftUI
import TwofaKit
import UniformTypeIdentifiers

/// The exported keychain, as the file `2fa` reads.
struct KeychainDocument: FileDocument {
    static let readableContentTypes: [UTType] = [.plainText]
    var text: String

    init(text: String) {
        self.text = text
    }

    init(configuration: ReadConfiguration) throws {
        text = String(decoding: configuration.file.regularFileContents ?? Data(), as: UTF8.self)
    }

    func fileWrapper(configuration: WriteConfiguration) throws -> FileWrapper {
        FileWrapper(regularFileWithContents: Data(text.utf8))
    }
}

struct KeysView: View {
    @Environment(AppModel.self) private var model

    /// The key being added: typed, or as scanned and waiting to be reviewed.
    @State private var draft: KeyDraft?
    @State private var scanning = false
    /// What the scanner read, until its sheet is gone.
    @State private var scanned: String?
    @State private var renaming: KeyItem?
    @State private var newName = ""
    @State private var removing: KeyItem?
    @State private var importing = false
    @State private var importSummary: String?
    @State private var confirmingExport = false
    @State private var exported: KeychainDocument?

    var body: some View {
        NavigationStack {
            List {
                if model.keys.contains(where: { !$0.isCounterBased }) {
                    Section {
                        ProgressView(value: Double(model.secondsRemaining), total: 30) {
                            Text("New codes in \(model.secondsRemaining) s")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .monospacedDigit()
                        }
                        .tint(model.secondsRemaining <= 5 ? Color.red : Color.accentColor)
                    }
                }
                Section {
                    ForEach(model.keys) { key in
                        KeyRowView(key: key, copied: model.copied == key.name) {
                            model.nextCode(for: key.name)
                        }
                        .contentShape(Rectangle())
                        .onTapGesture { model.copy(key.name) }
                        .accessibilityAddTraits(.isButton)
                        .accessibilityHint("Copies the code")
                        .swipeActions(edge: .trailing) {
                            Button("Delete", systemImage: "trash") { removing = key }
                                .tint(.red)
                            Button("Rename", systemImage: "pencil") {
                                newName = key.name
                                renaming = key
                            }
                        }
                    }
                } footer: {
                    if !model.skippedLines.isEmpty {
                        Text("\(model.skippedLines.count) lines of the imported file are not keys. They are kept, and exported as they are.")
                    }
                }
            }
            .overlay {
                if model.keys.isEmpty {
                    ContentUnavailableView(
                        "No keys",
                        systemImage: "key",
                        description: Text("Scan the QR code a service shows when you turn on two-factor authentication."))
                }
            }
            .navigationTitle("2fa")
            .toolbar { toolbar }
        }
        .task {
            // Once a second, on the second: the model says how long that is.
            while !Task.isCancelled {
                let wait = model.tick()
                try? await Task.sleep(for: .seconds(wait))
            }
        }
        .sheet(isPresented: $scanning, onDismiss: reviewScanned) {
            ScannerSheet { text in
                scanned = text
                scanning = false
            }
        }
        .sheet(
            isPresented: Binding(
                get: { draft != nil },
                set: { if !$0 { draft = nil } })
        ) {
            AddKeyView(draft: draft ?? KeyDraft())
        }
        .alert("Rename", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Name", text: $newName)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            Button("Rename") {
                if let renaming {
                    model.rename(renaming.name, to: newName)
                }
            }
            Button("Cancel", role: .cancel) {}
        }
        .confirmationDialog(
            "Delete \(removing?.name ?? "")?",
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }),
            titleVisibility: .visible
        ) {
            Button("Delete the key", role: .destructive) {
                if let removing {
                    model.remove(removing.name)
                }
            }
        } message: {
            Text("Its secret is erased from this device. Unless you exported it, or still have its QR code, the account it protects can lock you out.")
        }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.data]) { result in
            importFile(result)
        }
        .alert("Import", isPresented: Binding(get: { importSummary != nil }, set: { if !$0 { importSummary = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(importSummary ?? "")
        }
        .confirmationDialog("Export every key?", isPresented: $confirmingExport, titleVisibility: .visible) {
            Button("Export, not encrypted") {
                exported = model.exportText().map(KeychainDocument.init(text:))
            }
        } message: {
            Text("The file holds every secret in clear, in the format of the 2fa command line (~/.2fa). Whoever reads it can make your codes.")
        }
        .fileExporter(
            isPresented: Binding(get: { exported != nil }, set: { if !$0 { exported = nil } }),
            document: exported,
            contentType: .plainText,
            defaultFilename: "2fa"
        ) { _ in
            exported = nil
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItem(placement: .topBarLeading) {
            Menu {
                Button("Import a 2fa file", systemImage: "square.and.arrow.down") { importing = true }
                Button("Export", systemImage: "square.and.arrow.up") { confirmingExport = true }
                    .disabled(model.keys.isEmpty)
                Toggle(
                    "Ask for Face ID on open",
                    systemImage: "faceid",
                    isOn: Binding(
                        get: { model.lockOnOpen },
                        set: { enabled in Task { await model.setLockOnOpen(enabled) } }))
            } label: {
                Label("More", systemImage: "ellipsis.circle")
            }
        }
        ToolbarItem(placement: .topBarTrailing) {
            Menu {
                Button("Scan a QR code", systemImage: "qrcode.viewfinder") { scanning = true }
                Button("Enter a key", systemImage: "keyboard") { draft = KeyDraft() }
            } label: {
                Label("Add", systemImage: "plus")
            }
        }
    }

    /// After the scanner has closed: one sheet at a time.
    private func reviewScanned() {
        guard let text = scanned else { return }
        scanned = nil
        draft = model.draft(fromURI: text)
    }

    private func importFile(_ result: Result<URL, Error>) {
        do {
            let url = try result.get()
            let scoped = url.startAccessingSecurityScopedResource()
            defer {
                if scoped { url.stopAccessingSecurityScopedResource() }
            }
            importSummary = model.importFile(try Data(contentsOf: url))?.summary
        } catch {
            model.errorMessage = message(for: error)
        }
    }
}

struct KeyRowView: View {
    let key: KeyItem
    let copied: Bool
    let next: () -> Void

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(key.name)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Text(key.code.map(groupedCode) ?? String(repeating: "–", count: key.digits))
                    .font(.title.monospacedDigit())
                    .foregroundStyle(key.code == nil ? .secondary : .primary)
            }
            Spacer()
            if copied {
                Label("Copied", systemImage: "checkmark.circle.fill")
                    .labelStyle(.iconOnly)
                    .foregroundStyle(.green)
            }
            if key.isCounterBased {
                // Borderless: a button of its own in the row, not the row's tap.
                Button(action: next) {
                    Label("Next code", systemImage: "arrow.clockwise.circle")
                        .labelStyle(.iconOnly)
                        .font(.title2)
                }
                .buttonStyle(.borderless)
            }
        }
    }
}
