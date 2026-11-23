import PassKit
import SwiftUI
import UniformTypeIdentifiers

/// The setup, in the order it is needed (this device's key, the drop zone, the other devices),
/// and the settings afterwards. Who reads what is not here: that is on each folder of the store.
struct SettingsView: View {
    @Environment(AppModel.self) private var model

    @State private var draft = PassSettings()
    @State private var busy = false
    @State private var confirmingErase = false

    var body: some View {
        Form {
            KeySection()
            if model.keyInfo != nil {
                DropZoneSection(draft: $draft, busy: $busy)
                PeersSection()
                if let problem = model.configurationProblem {
                    Section {
                        Label(problem, systemImage: "exclamationmark.triangle").foregroundStyle(.red)
                    }
                }
                safetySection
                Section {
                    Button("Save") { model.save(draft) }
                        .disabled(busy || draft == model.settings)
                }
            }
            Section {
                Button("Erase everything from this device", role: .destructive) {
                    confirmingErase = true
                }
            } footer: {
                Text("The store, the keys and the settings. A key made on this device is gone for good, and what was not published is lost. Nothing is removed in the drop zone.")
            }
        }
        .navigationTitle(title)
        .onAppear { draft = model.settings }
        // Adding a device or making a key saves on its own: the draft follows.
        .onChange(of: model.settings) { _, saved in draft = saved }
        .confirmationDialog("Erase the store and the keys from this device?", isPresented: $confirmingErase, titleVisibility: .visible) {
            Button("Erase", role: .destructive) { model.eraseEverything() }
        }
    }

    private var title: String {
        switch model.stage {
        case .needsKey: return "1. This device's key"
        case .needsDropZone: return "2. The sync"
        case .needsStore, .ready: return "Settings"
        }
    }

    private var safetySection: some View {
        Section("Safety") {
            Stepper("Lock after \(Int(draft.lockAfter)) s without use", value: $draft.lockAfter, in: 15...900, step: 15)
            Stepper("Clipboard cleared after \(Int(draft.clipboardExpiry)) s", value: $draft.clipboardExpiry, in: 10...120, step: 5)
        }
    }
}

/// This device's OpenPGP key: made here (its private half then never exists anywhere else), or imported.
private struct KeySection: View {
    @Environment(AppModel.self) private var model

    @State private var importing = false
    @State private var armored = ""
    @State private var passphrase = ""
    @State private var faceID = true
    @State private var pickingFile = false
    @State private var busy = false

    var body: some View {
        Section {
            if let info = model.keyInfo {
                LabeledContent("Key", value: info.userIds.first ?? "")
                Text(groupedFingerprint(info.fingerprint))
                    .font(.caption.monospaced())
                    .textSelection(.enabled)
                LabeledContent("Unlocked by", value: model.canUnlockWithBiometrics ? "Face ID" : "its passphrase, each time")
                if let publicKey = model.ownPublicKey {
                    ShareLink("Send the public key to the other devices", item: publicKey)
                }
            } else {
                Toggle("Import a key made elsewhere", isOn: $importing)
                if importing {
                    TextEditor(text: $armored)
                        .font(.caption.monospaced())
                        .frame(minHeight: 100)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    Button("Choose the exported key file") { pickingFile = true }
                }
                SecureField(importing ? "Its passphrase" : "A passphrase for the new key", text: $passphrase)
                Toggle("Unlock with Face ID", isOn: $faceID)
                Button {
                    Task { await go() }
                } label: {
                    HStack {
                        Text(importing ? "Import" : "Make this device's key")
                        Spacer()
                        if busy { ProgressView() }
                    }
                }
                .disabled(busy || (importing && armored.isEmpty))
            }
        } header: {
            Text("OpenPGP key")
        } footer: {
            if model.keyInfo != nil {
                Text("On a computer: gpg --import the public key, compare the fingerprint above with what gpg shows, then pass sync peer \(model.settings.deviceName) <fingerprint>. Being a device is not reading: a folder is read by the devices it names.")
            } else {
                Text("A key made here is this device's alone: the others only ever get its public half. With Face ID, the passphrase is kept in this device's keychain; without, it is typed at each unlock and kept nowhere.")
            }
        }
        .fileImporter(isPresented: $pickingFile, allowedContentTypes: [.data, .text]) { result in
            guard case .success(let url) = result else { return }
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            armored = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
        }
    }

    private func go() async {
        busy = true
        defer { busy = false }
        let policy: PassphrasePolicy = faceID ? .keychainWithBiometrics : .askEachTime
        if importing {
            if await model.importSecretKey(armored, passphrase: passphrase, policy: policy) {
                armored = ""
            }
        } else {
            await model.makeKey(passphrase: passphrase, policy: policy)
        }
        passphrase = ""
    }
}

/// Where the devices leave their files: its address, its pinned host key, and this device's SSH key for it.
private struct DropZoneSection: View {
    @Binding var draft: PassSettings
    @Binding var busy: Bool
    @Environment(AppModel.self) private var model

    /// A host key read from the server, waiting to be compared and trusted.
    @State private var offered: HostKey?

    var body: some View {
        Section {
            TextField("Name of this device", text: $draft.deviceName)
            TextField("sftp://drop@your-server:2222", text: $draft.dropZoneURL)
                .keyboardType(.URL)
                .textContentType(.URL)
        } header: {
            Text("Drop zone")
        } footer: {
            Text("The server only holds one encrypted file per device (see dropzone/README.md). The name is how the other devices know this one: lowercase letters, digits, hyphens.")
        }
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        // A pinned host key belongs to the server it was read from.
        .onChange(of: draft.dropZoneURL) { old, new in
            if !old.isEmpty && old != new {
                draft.dropZoneHostKey = nil
                offered = nil
            }
        }
        Section {
            if let key = model.deviceKey {
                Text(key.publicKey)
                    .font(.caption2.monospaced())
                    .textSelection(.enabled)
                ShareLink("Send it to whoever runs the drop zone", item: key.publicKey)
            } else {
                Button("Make this device's SSH key") { model.makeDeviceKey() }
            }
        } header: {
            Text("This device's SSH key")
        } footer: {
            Text("It only lets this device into the drop zone: DROPZONE_KEY_\(draft.deviceName.uppercased().replacingOccurrences(of: "-", with: "_")). The private half never leaves this device.")
        }
        Section {
            if let pinned = draft.dropZoneHostKey {
                LabeledContent("Pinned") {
                    Text(pinned).font(.caption.monospaced()).multilineTextAlignment(.trailing)
                }
            }
            if let offered {
                VStack(alignment: .leading, spacing: 8) {
                    Text("The server presents (\(offered.algorithm)):")
                    Text(offered.fingerprint)
                        .font(.callout.monospaced())
                        .textSelection(.enabled)
                    Button("It matches: trust this host key") {
                        draft.dropZoneHostKey = offered.fingerprint
                        self.offered = nil
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
            Button(draft.dropZoneHostKey == nil ? "Read the server's host key" : "Read it again") {
                Task {
                    busy = true
                    defer { busy = false }
                    do {
                        offered = try await readHostKey(url: draft.dropZoneURL)
                    } catch {
                        model.errorMessage = message(for: error)
                    }
                }
            }
            .disabled(busy || draft.dropZoneURL.isEmpty)
        } header: {
            Text("The drop zone's host key")
        } footer: {
            Text("Compare with what the drop zone printed when it started (docker logs pass-dropzone). Nothing is sent to a server whose key is not the pinned one.")
        }
    }
}

/// The other devices: equals, each known by a name and its public key.
private struct PeersSection: View {
    @Environment(AppModel.self) private var model

    @State private var name = ""
    @State private var publicKey = ""

    var body: some View {
        Section {
            ForEach(model.settings.peers) { peer in
                VStack(alignment: .leading, spacing: 4) {
                    Text(peer.name).font(.headline)
                    Text(groupedFingerprint(peer.fingerprint))
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                }
                .swipeActions {
                    Button("Remove", role: .destructive) { model.removePeer(named: peer.name) }
                }
            }
            TextField("Name (pc)", text: $name)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            TextEditor(text: $publicKey)
                .font(.caption.monospaced())
                .frame(minHeight: 60)
            Button("Add this device") {
                if model.addPeer(name: name, publicKey: publicKey) {
                    name = ""
                    publicKey = ""
                }
            }
            .disabled(name.isEmpty || publicKey.isEmpty)
        } header: {
            Text("Devices")
        } footer: {
            Text("Each by its public key (gpg --export --armor <id>), under the name it gave itself. Compare the fingerprint shown here with the one that device shows of itself: it is all that says a published file is really from it. Nothing received can add a device or change a key. Every device must know every other: a store that names a key this one does not know is refused.")
        }
    }
}
