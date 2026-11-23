import Foundation
import Observation

/// A file on its way to the server.
public struct UploadInProgress: Identifiable, Equatable, Sendable {
    public let id: UUID
    public let name: String
    public var fraction: Double
}

/// Everything the views show, and everything they can ask for. No view talks to the server itself.
@MainActor
@Observable
public final class AppModel {
    public private(set) var settings: ServerSettings
    public private(set) var secret: String
    /// `nil` until the settings are complete and valid.
    public private(set) var service: CompanionService?

    /// Newest first.
    public private(set) var files: [RemoteFile] = []
    public private(set) var uploads: [UploadInProgress] = []
    /// File id → fraction, while it downloads.
    public private(set) var downloads: [String: Double] = [:]
    public private(set) var preview: PreviewState?

    /// What the pad editor shows. Edited by the user, and replaced by the server's when they have no unsent edit.
    public var padText = ""
    /// The server's pad, as last seen.
    private var padSynced = ""
    private var padTimestamp: UInt64 = 0

    /// The last failure, for an alert. Set to `nil` to dismiss.
    public var errorMessage: String?

    private let store: SettingsStore
    private let downloadDirectory: URL

    public init(store: SettingsStore, downloadDirectory: URL) {
        self.store = store
        self.downloadDirectory = downloadDirectory
        (settings, secret) = store.load()
        service = settings.isComplete ? try? CompanionService(settings: settings, secret: secret) : nil
    }

    public var isConnected: Bool { service != nil }
    /// The pad has edits the server has not got yet.
    public var padIsDirty: Bool { padText != padSynced }

    private func fail(_ error: Error) {
        errorMessage = message(for: error)
    }

    // MARK: Settings

    /// Checks the settings against the server (lists its files) and only then saves them.
    /// Returns whether it worked; the reason is in `errorMessage` when it did not.
    @discardableResult
    public func connect(settings newSettings: ServerSettings, secret newSecret: String) async -> Bool {
        do {
            let candidate = try CompanionService(settings: newSettings, secret: newSecret)
            let listed = try await candidate.files()
            try store.save(newSettings, secret: newSecret)
            if newSettings != settings {
                // Another server: nothing of the previous one applies.
                preview = nil
                padText = ""
                padSynced = ""
                padTimestamp = 0
            }
            (settings, secret, service, files) = (newSettings, newSecret, candidate, listed)
            return true
        } catch {
            fail(error)
            return false
        }
    }

    // MARK: Files

    /// `quiet`: for the background refresh, which must not raise an alert every few seconds while offline.
    public func refreshFiles(quiet: Bool = false) async {
        guard let service else { return }
        do {
            files = try await service.files()
        } catch {
            if !quiet { fail(error) }
        }
    }

    /// Uploads one file and refreshes the list. `cleanUp` removes a temporary copy afterwards.
    public func upload(fileAt url: URL, named name: String? = nil, cleanUp: Bool = false) async {
        defer {
            if cleanUp { try? FileManager.default.removeItem(at: url) }
        }
        guard let service else { return }
        let id = UUID()
        uploads.append(UploadInProgress(id: id, name: name ?? url.lastPathComponent, fraction: 0))
        defer { uploads.removeAll { $0.id == id } }
        do {
            _ = try await service.upload(fileAt: url, named: name) { fraction in
                Task { @MainActor [weak self] in
                    guard let self, let index = self.uploads.firstIndex(where: { $0.id == id }) else { return }
                    self.uploads[index].fraction = fraction
                }
            }
            await refreshFiles()
        } catch {
            fail(error)
        }
    }

    /// Downloads a file (unless it is already here) and returns where it is, to preview or share it.
    public func localCopy(of file: RemoteFile) async -> URL? {
        guard let service else { return nil }
        downloads[file.id] = 0
        defer { downloads[file.id] = nil }
        do {
            let id = file.id
            return try await service.download(file, into: downloadDirectory) { fraction in
                Task { @MainActor [weak self] in
                    guard let self, self.downloads[id] != nil else { return }
                    self.downloads[id] = fraction
                }
            }
        } catch {
            fail(error)
            return nil
        }
    }

    /// Removes the downloaded copies. Nothing is deleted on the server.
    public func clearDownloads() {
        try? FileManager.default.removeItem(at: downloadDirectory)
    }

    // MARK: Presentation

    /// Asks every client to show this file.
    public func present(_ file: RemoteFile) async {
        guard let service else { return }
        do {
            try await service.present(fileId: file.id)
            await refreshPreview()
        } catch {
            fail(error)
        }
    }

    /// Returns whether what is presented has changed since the last call.
    @discardableResult
    public func refreshPreview() async -> Bool {
        guard let service, let latest = try? await service.preview() else { return false }
        let changed = latest.timestamp != preview?.timestamp
        preview = latest
        return changed
    }

    /// The presented file, once it is in the list (fetched again when it is not there yet).
    public func presentedFile() async -> RemoteFile? {
        guard let id = preview?.fileId else { return nil }
        if let known = files.first(where: { $0.id == id }) {
            return known
        }
        await refreshFiles(quiet: true)
        return files.first { $0.id == id }
    }

    // MARK: Pad

    /// Takes the server's pad, unless there are local edits still to send: those win (as in the web UI, last write wins).
    public func pullPad() async {
        guard let service, let pad = try? await service.pad() else { return }
        guard pad.timestamp != padTimestamp else { return }
        let dirty = padIsDirty // after the await: the user may have typed meanwhile
        padTimestamp = pad.timestamp
        padSynced = pad.content
        if !dirty {
            padText = pad.content
        }
    }

    /// Sends the local edits, if any.
    public func pushPad() async {
        guard let service, padIsDirty else { return }
        let sending = padText
        do {
            padTimestamp = try await service.setPad(sending)
            padSynced = sending
        } catch {
            fail(error)
        }
    }
}
