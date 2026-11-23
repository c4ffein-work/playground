import CompanionKit
import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// "Companion" in the share sheet of any app: files go to the server, links and text to the pad.
final class ShareViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        let items = extensionContext?.inputItems as? [NSExtensionItem] ?? []
        let model = ShareModel(providers: items.flatMap { $0.attachments ?? [] })
        let host = UIHostingController(rootView: ShareView(model: model) { [weak self] in
            self?.extensionContext?.completeRequest(returningItems: nil)
        })
        addChild(host)
        host.view.frame = view.bounds
        host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(host.view)
        host.didMove(toParent: self)
    }
}

@MainActor
@Observable
final class ShareModel {
    enum State: Equatable {
        case waiting
        case sending(Double)
        case done(String)
        case failed(String)
    }

    struct Entry: Identifiable {
        let id = UUID()
        var title: String
        var state: State = .waiting
    }

    private(set) var entries: [Entry]
    private(set) var finished = false
    private let providers: [NSItemProvider]

    init(providers: [NSItemProvider]) {
        self.providers = providers
        entries = providers.map { Entry(title: $0.suggestedName ?? "Item") }
    }

    var succeeded: Bool {
        finished && entries.allSatisfy { if case .done = $0.state { true } else { false } }
    }

    func run() async {
        defer { finished = true }
        let service: CompanionService
        do {
            let (settings, secret) = SettingsStore.shared.load()
            guard settings.isComplete else {
                throw ShareError("Open Companion once to set the server up.")
            }
            service = try CompanionService(settings: settings, secret: secret)
        } catch {
            for index in entries.indices {
                entries[index].state = .failed(message(for: error))
            }
            return
        }
        // One at a time: a share extension has little memory, and the server holds uploads in its own.
        for (index, provider) in providers.enumerated() {
            do {
                let outcome = try await send(provider, at: index, with: service)
                entries[index].state = .done(outcome)
            } catch {
                entries[index].state = .failed(message(for: error))
            }
        }
    }

    private func send(_ provider: NSItemProvider, at index: Int, with service: CompanionService) async throws -> String {
        let isFile = provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier)
        if !isFile, provider.hasItemConformingToTypeIdentifier(UTType.url.identifier) {
            let url = try await provider.loadURL()
            entries[index].title = url.absoluteString
            try await service.appendToPad(url.absoluteString)
            return "Added to the pad"
        }
        if !isFile, provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) {
            let text = try await provider.loadText()
            entries[index].title = String(text.prefix(60))
            try await service.appendToPad(text)
            return "Added to the pad"
        }
        let copy = try await provider.copyFile()
        defer { try? FileManager.default.removeItem(at: copy.deletingLastPathComponent()) }
        entries[index].title = copy.lastPathComponent
        entries[index].state = .sending(0)
        let uploaded = try await service.upload(fileAt: copy) { fraction in
            Task { @MainActor [weak self] in
                self?.entries[index].state = .sending(fraction)
            }
        }
        return "Uploaded as \(uploaded.filename)"
    }
}

struct ShareError: LocalizedError {
    let errorDescription: String?

    init(_ text: String) {
        errorDescription = text
    }
}

extension NSItemProvider {
    func loadURL() async throws -> URL {
        guard let url = try await loadItem(forTypeIdentifier: UTType.url.identifier) as? URL else {
            throw ShareError("Could not read the link.")
        }
        return url
    }

    func loadText() async throws -> String {
        guard let text = try await loadItem(forTypeIdentifier: UTType.plainText.identifier) as? String else {
            throw ShareError("Could not read the text.")
        }
        return text
    }

    /// The shared file only exists during the callback: copy it where it can be uploaded from.
    func copyFile() async throws -> URL {
        let type = registeredTypeIdentifiers.first { $0 != UTType.fileURL.identifier } ?? UTType.data.identifier
        return try await withCheckedThrowingContinuation { continuation in
            _ = loadFileRepresentation(forTypeIdentifier: type) { url, error in
                guard let url else {
                    continuation.resume(throwing: error ?? ShareError("Could not read the file."))
                    return
                }
                do {
                    let directory = URL.temporaryDirectory.appending(path: UUID().uuidString)
                    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                    let copy = directory.appending(path: url.lastPathComponent)
                    try FileManager.default.copyItem(at: url, to: copy)
                    continuation.resume(returning: copy)
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }
}

struct ShareView: View {
    let model: ShareModel
    let close: () -> Void

    var body: some View {
        NavigationStack {
            List(model.entries) { entry in
                VStack(alignment: .leading, spacing: 4) {
                    Text(entry.title).lineLimit(2)
                    switch entry.state {
                    case .waiting:
                        Text("Waiting").font(.caption).foregroundStyle(.secondary)
                    case .sending(let fraction):
                        ProgressView(value: fraction)
                    case .done(let text):
                        Label(text, systemImage: "checkmark.circle.fill").font(.caption).foregroundStyle(.green)
                    case .failed(let text):
                        Label(text, systemImage: "xmark.octagon.fill").font(.caption).foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("Companion")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(model.finished ? "Done" : "Cancel", action: close)
                }
            }
            .task {
                await model.run()
                // All went through: nothing to read, get out of the way.
                if model.succeeded {
                    try? await Task.sleep(for: .seconds(1))
                    close()
                }
            }
        }
    }
}
