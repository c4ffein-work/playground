import BankKit
import PhotosUI
import QuickLook
import SwiftUI
import UniformTypeIdentifiers

/// One transaction: what it is, the receipts it has, and a way to add one.
struct TransactionDetailView: View {
    @Environment(AppModel.self) private var model
    let transaction: Transaction

    @State private var opened: URL?
    @State private var takingPhoto = false
    @State private var pickingPhoto = false
    @State private var pickingFile = false
    @State private var photo: PhotosPickerItem?

    var body: some View {
        List {
            Section {
                VStack(alignment: .leading, spacing: 4) {
                    Text(transaction.amount.formatted())
                        .font(.largeTitle.monospacedDigit())
                    if let local = transaction.localAmount {
                        Text(local.formatted())
                            .font(.callout.monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                }
                if let date = transaction.emittedDate {
                    LabeledContent("Date", value: date.formatted(date: .long, time: .shortened))
                }
                if let date = transaction.settledDate {
                    LabeledContent("Settled", value: date.formatted(date: .long, time: .omitted))
                }
                row("Counterparty", transaction.cleanCounterpartyName)
                row("Reference", transaction.reference)
                row("Note", transaction.note)
                row("Card", transaction.cardLastDigits.map { "… \($0)" })
                row("Type", transaction.operationType)
                row("Status", transaction.status)
                // What `bank justify` takes on the desktop.
                LabeledContent("ID", value: transaction.shortId)
            }

            Section {
                if let fraction = model.uploadFraction(for: transaction) {
                    ProgressView(value: fraction) {
                        Text("Sending the receipt")
                    }
                }
                if let attachments = model.attachments[transaction.id] {
                    ForEach(Array(attachments.enumerated()), id: \.element.id) { index, attachment in
                        AttachmentRow(
                            attachment: attachment, number: index + 1,
                            fraction: model.downloads[attachment.id]
                        ) {
                            Task { opened = await model.localCopy(of: attachment, in: transaction) }
                        }
                    }
                    if attachments.isEmpty {
                        Text("No receipt yet")
                            .foregroundStyle(.secondary)
                    }
                } else {
                    ProgressView()
                }
                Menu {
                    Button {
                        takingPhoto = true
                    } label: {
                        Label("Take a photo", systemImage: "camera")
                    }
                    .disabled(!cameraIsAvailable)
                    Button {
                        pickingPhoto = true
                    } label: {
                        Label("Photo library", systemImage: "photo.on.rectangle")
                    }
                    Button {
                        pickingFile = true
                    } label: {
                        Label("Files", systemImage: "folder")
                    }
                } label: {
                    Label("Attach a receipt", systemImage: "paperclip")
                }
                .disabled(model.uploadFraction(for: transaction) != nil)
            } header: {
                Text("Receipts")
            } footer: {
                Text("PDF, JPEG or PNG. A photo in another format is converted to JPEG first.")
            }
        }
        .navigationTitle(transaction.title)
        .navigationBarTitleDisplayMode(.inline)
        .task { await model.loadAttachments(of: transaction) }
        .refreshable { await model.loadAttachments(of: transaction) }
        .quickLookPreview($opened)
        .fullScreenCover(isPresented: $takingPhoto) {
            CameraPicker { image in
                takingPhoto = false
                if let data = image?.jpegData(compressionQuality: 0.85) {
                    Task { await attach(data) }
                }
            }
            .ignoresSafeArea()
        }
        .photosPicker(isPresented: $pickingPhoto, selection: $photo, matching: .images)
        .onChange(of: photo) { _, picked in
            guard let picked else { return }
            photo = nil
            Task {
                do {
                    guard let data = try await picked.loadTransferable(type: Data.self) else { return }
                    await attach(data)
                } catch {
                    model.errorMessage = message(for: error)
                }
            }
        }
        .fileImporter(isPresented: $pickingFile, allowedContentTypes: [.pdf, .jpeg, .png]) { result in
            guard case .success(let url) = result else { return }
            Task { await attach(fileAt: url) }
        }
    }

    @ViewBuilder
    private func row(_ title: String, _ value: String?) -> some View {
        if let value, !value.isEmpty {
            LabeledContent(title, value: value)
        }
    }

    /// A photo: written to a temporary file (converted to JPEG when it is not what Qonto accepts), sent, removed.
    private func attach(_ data: Data) async {
        do {
            let file = try prepareReceipt(data, convert: jpegData(from:))
            await model.attach(fileAt: file, to: transaction, cleanUp: true)
        } catch {
            model.errorMessage = message(for: error)
        }
    }

    /// A file picked in the Files app is only readable inside its security scope.
    private func attach(fileAt url: URL) async {
        let scoped = url.startAccessingSecurityScopedResource()
        await model.attach(fileAt: url, to: transaction)
        if scoped {
            url.stopAccessingSecurityScopedResource()
        }
    }
}

struct AttachmentRow: View {
    let attachment: Attachment
    let number: Int
    /// Set while it downloads.
    let fraction: Double?
    let open: () -> Void

    var body: some View {
        Button(action: open) {
            HStack(spacing: 12) {
                Image(systemName: attachment.symbolName)
                    .foregroundStyle(.tint)
                    .frame(width: 28)
                VStack(alignment: .leading, spacing: 2) {
                    // The name the API gives is the one it was sent with: from here and the CLI, "file.pdf".
                    Text("Receipt \(number)")
                        .foregroundStyle(.primary)
                    if let date = attachment.createdDate {
                        Text(date.formatted(date: .abbreviated, time: .shortened))
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    if let fraction {
                        ProgressView(value: fraction)
                    }
                }
            }
        }
        .disabled(fraction != nil)
    }
}
