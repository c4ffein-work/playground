import CompanionKit
import PhotosUI
import QuickLook
import SwiftUI
import UniformTypeIdentifiers

/// A photo or a video picked from the library, copied where it can be uploaded from.
struct PickedMedia: Transferable {
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(importedContentType: .movie) { try PickedMedia(copying: $0.file) }
        FileRepresentation(importedContentType: .image) { try PickedMedia(copying: $0.file) }
    }

    /// The received file only lives for the duration of the import.
    init(copying file: URL) throws {
        let directory = URL.temporaryDirectory.appending(path: UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        url = directory.appending(path: file.lastPathComponent)
        try FileManager.default.copyItem(at: file, to: url)
    }
}

struct FilesView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase

    @State private var pickingPhotos = false
    @State private var pickingFiles = false
    @State private var photos: [PhotosPickerItem] = []
    @State private var opened: URL?

    var body: some View {
        NavigationStack {
            List {
                if !model.uploads.isEmpty {
                    Section("Uploading") {
                        ForEach(model.uploads) { upload in
                            ProgressView(value: upload.fraction) {
                                Text(upload.name).lineLimit(1)
                            }
                        }
                    }
                }
                Section {
                    ForEach(model.files) { file in
                        FileRow(file: file, fraction: model.downloads[file.id]) {
                            Task { opened = await model.localCopy(of: file) }
                        }
                        .swipeActions(edge: .trailing) {
                            Button {
                                Task { await model.present(file) }
                            } label: {
                                Label("Present", systemImage: "play.rectangle")
                            }
                            .tint(.indigo)
                        }
                    }
                }
            }
            .overlay {
                if model.files.isEmpty && model.uploads.isEmpty {
                    ContentUnavailableView(
                        "No files", systemImage: "tray",
                        description: Text("Add one with +, or share to Companion from any app."))
                }
            }
            .navigationTitle("Files")
            .toolbar {
                Menu {
                    Button {
                        pickingPhotos = true
                    } label: {
                        Label("Photos and videos", systemImage: "photo.on.rectangle")
                    }
                    Button {
                        pickingFiles = true
                    } label: {
                        Label("Files", systemImage: "folder")
                    }
                } label: {
                    Label("Add", systemImage: "plus")
                }
            }
            .refreshable { await model.refreshFiles() }
            .photosPicker(
                isPresented: $pickingPhotos, selection: $photos, maxSelectionCount: 20,
                matching: .any(of: [.images, .videos]))
            .onChange(of: photos) { _, picked in
                guard !picked.isEmpty else { return }
                photos = []
                Task { await upload(picked) }
            }
            .fileImporter(isPresented: $pickingFiles, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
                guard case .success(let urls) = result else { return }
                Task { await upload(urls) }
            }
            .quickLookPreview($opened)
            // The list changes when other devices upload: follow it while it is on screen.
            .task(id: scenePhase) {
                guard scenePhase == .active else { return }
                while !Task.isCancelled {
                    await model.refreshFiles(quiet: true)
                    try? await Task.sleep(for: .seconds(5))
                }
            }
        }
    }

    private func upload(_ picked: [PhotosPickerItem]) async {
        for item in picked {
            do {
                guard let media = try await item.loadTransferable(type: PickedMedia.self) else { continue }
                await model.upload(fileAt: media.url, cleanUp: true)
            } catch {
                model.errorMessage = message(for: error)
            }
        }
    }

    /// Files picked in the Files app are only readable inside their security scope.
    private func upload(_ urls: [URL]) async {
        for url in urls {
            let scoped = url.startAccessingSecurityScopedResource()
            await model.upload(fileAt: url)
            if scoped {
                url.stopAccessingSecurityScopedResource()
            }
        }
    }
}

struct FileRow: View {
    let file: RemoteFile
    /// Set while the file downloads.
    let fraction: Double?
    let open: () -> Void

    var body: some View {
        Button(action: open) {
            HStack(spacing: 12) {
                Image(systemName: file.symbolName)
                    .font(.title2)
                    .foregroundStyle(.tint)
                    .frame(width: 32)
                VStack(alignment: .leading, spacing: 2) {
                    Text(file.name)
                        .foregroundStyle(.primary)
                        .lineLimit(2)
                    Text(details)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    if let fraction {
                        ProgressView(value: fraction)
                    }
                }
            }
        }
        .disabled(fraction != nil)
    }

    private var details: String {
        var parts = [file.sizeText]
        if let date = file.uploadedDate {
            parts.append(date.formatted(.relative(presentation: .named)))
        }
        if !file.uploadedBy.isEmpty {
            parts.append(file.uploadedBy)
        }
        return parts.joined(separator: " · ")
    }
}
