import PassKit
import SwiftUI
import UniformTypeIdentifiers

/// The setup, in the order it is needed (the key, the relay, the clone), and the settings afterwards.
struct SettingsView: View {
    @Environment(AppModel.self) private var model

    @State private var draft = PassSettings()
    @State private var busy = false
    @State private var confirmingErase = false

    var body: some View {
        Form {
            KeySection()
            if model.keyInfo != nil {
                relaySection
                switch draft.transport {
                case .ssh: SSHSection(draft: $draft, busy: $busy)
                case .https: HTTPSSection(draft: $draft, busy: $busy)
                }
                saveSection
            }
            if model.stage == .ready {
                RecipientsSection()
                safetySection
            }
            Section {
                Button("Erase everything from this device", role: .destructive) {
                    confirmingErase = true
                }
            } footer: {
                Text("The store, the keys and the settings. Nothing is removed on the relay; changes not synced yet are lost.")
            }
        }
        .navigationTitle(title)
        .onAppear { draft = model.settings }
        .confirmationDialog("Erase the store and the keys from this device?", isPresented: $confirmingErase, titleVisibility: .visible) {
            Button("Erase", role: .destructive) {
                model.eraseEverything()
                draft = model.settings
            }
        }
    }

    private var title: String {
        switch model.stage {
        case .needsKey: return "1. Your key"
        case .needsRemote: return "2. The relay"
        case .needsClone: return "3. The store"
        case .ready: return "Settings"
        }
    }

    private var relaySection: some View {
        Section {
            Picker("Reached over", selection: $draft.transport) {
                Text("SSH").tag(Transport.ssh)
                Text("HTTPS").tag(Transport.https)
            }
            .pickerStyle(.segmented)
            TextField(draft.transport == .ssh ? "git@relay.example:store.git" : "https://git.example/store.git", text: $draft.url)
                .keyboardType(.URL)
                .textContentType(.URL)
            TextField("Name of this device", text: $draft.deviceName)
            TextField("Author of its commits (name)", text: $draft.authorName)
            TextField("Author of its commits (email)", text: $draft.authorEmail)
                .keyboardType(.emailAddress)
        } header: {
            Text("Relay")
        } footer: {
            Text("The git repository this device and the computer both sync with. This device only ever pushes to its own branch, devices/\(draft.deviceName).")
        }
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        // A pinned host key or certificate belongs to the server it was read from.
        .onChange(of: draft.url) { old, new in
            if !old.isEmpty && old != new {
                draft.sshHostKey = nil
                draft.pinSHA256 = nil
            }
        }
    }

    private var saveSection: some View {
        Section {
            Button("Save") {
                model.save(draft)
            }
            .disabled(busy || draft == model.settings)
            if model.stage == .needsClone {
                Button {
                    Task {
                        busy = true
                        defer { busy = false }
                        await model.cloneStore()
                    }
                } label: {
                    HStack {
                        Text("Clone the store")
                        Spacer()
                        if model.isSyncing { ProgressView() }
                    }
                }
                .disabled(busy || draft != model.settings)
            }
        }
    }

    private var safetySection: some View {
        Section {
            Toggle("Only take signed updates", isOn: $draft.requireSignedUpdates)
            TextField(
                "Fingerprints that must sign .gpg-id",
                text: Binding(
                    get: { draft.signingKeys.joined(separator: " ") },
                    set: { draft.signingKeys = $0.split(separator: " ").map(String.init) }))
                .font(.caption.monospaced())
                .textInputAutocapitalization(.characters)
                .autocorrectionDisabled()
            Stepper("Lock after \(Int(draft.lockAfter)) s without use", value: $draft.lockAfter, in: 15...900, step: 15)
            Stepper("Clipboard cleared after \(Int(draft.clipboardExpiry)) s", value: $draft.clipboardExpiry, in: 10...120, step: 5)
        } header: {
            Text("Safety")
        } footer: {
            Text("Signed updates: the last commit of what the relay offers must be signed by a key of the store (git config commit.gpgsign true on the computer). The relay can then neither add nor remove anything unseen. The fingerprints are pass's PASSWORD_STORE_SIGNING_KEY: with them, a .gpg-id is only used when its .gpg-id.sig is from one of these keys.")
        }
    }
}

/// The OpenPGP secret key: imported once, kept in this device's keychain as exported.
private struct KeySection: View {
    @Environment(AppModel.self) private var model

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
            } else {
                TextEditor(text: $armored)
                    .font(.caption.monospaced())
                    .frame(minHeight: 100)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                Button("Choose the exported key file") { pickingFile = true }
                SecureField("Its passphrase", text: $passphrase)
                Toggle("Unlock with Face ID", isOn: $faceID)
                Button {
                    Task { await importKey() }
                } label: {
                    HStack {
                        Text("Import")
                        Spacer()
                        if busy { ProgressView() }
                    }
                }
                .disabled(busy || armored.isEmpty)
            }
        } header: {
            Text("OpenPGP key")
        } footer: {
            if model.keyInfo == nil {
                Text("On the computer: gpg --export-secret-keys --armor <id>, brought here by AirDrop or a cable, not by mail. With Face ID, the passphrase is kept in this device's keychain; without, it is typed at each unlock and kept nowhere.")
            }
        }
        .fileImporter(isPresented: $pickingFile, allowedContentTypes: [.data, .text]) { result in
            guard case .success(let url) = result else { return }
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            armored = (try? String(contentsOf: url, encoding: .utf8)) ?? ""
        }
    }

    private func importKey() async {
        busy = true
        defer { busy = false }
        let policy: PassphrasePolicy = faceID ? .keychainWithBiometrics : .askEachTime
        if await model.importSecretKey(armored, passphrase: passphrase, policy: policy) {
            armored = ""
        }
        passphrase = ""
    }
}

/// SSH: this device's key for the relay, and the relay's host key for this device.
private struct SSHSection: View {
    @Binding var draft: PassSettings
    @Binding var busy: Bool
    @Environment(AppModel.self) private var model

    /// A host key read from the server, waiting to be compared and trusted.
    @State private var offered: HostKey?

    var body: some View {
        Section {
            if let line = model.authorizedKeysLine {
                Text(line)
                    .font(.caption2.monospaced())
                    .textSelection(.enabled)
                ShareLink("Send this line to the computer", item: line)
            } else {
                Button("Make this device's key") { model.makeDeviceKey() }
            }
        } header: {
            Text("This device's key")
        } footer: {
            Text("One line for ~/.ssh/authorized_keys of the relay's account. With the relay's update hook, this key can push to devices/\(draft.deviceName) and nowhere else. The private half never leaves this device.")
        }
        Section {
            if let pinned = draft.sshHostKey {
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
                        draft.sshHostKey = offered.fingerprint
                        self.offered = nil
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
            Button(draft.sshHostKey == nil ? "Read the server's host key" : "Read it again") {
                Task {
                    busy = true
                    defer { busy = false }
                    do {
                        offered = try await readHostKey(url: draft.url)
                    } catch {
                        model.errorMessage = message(for: error)
                    }
                }
            }
            .disabled(busy || draft.url.isEmpty)
        } header: {
            Text("The relay's host key")
        } footer: {
            Text("Compare with what the relay itself prints: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub. Nothing is sent to a server whose key is not the pinned one.")
        }
    }
}

/// HTTPS: a token, and how the certificate is trusted.
private struct HTTPSSection: View {
    @Binding var draft: PassSettings
    @Binding var busy: Bool
    @Environment(AppModel.self) private var model

    @State private var token = ""
    /// A certificate read from the server, waiting to be compared and trusted.
    @State private var offered: CertificateFingerprint?

    var body: some View {
        Section {
            TextField("User name", text: $draft.httpsUsername)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            SecureField(model.hasHTTPSToken ? "Token (saved: type to replace)" : "Token", text: $token)
                .onSubmit {
                    model.setHTTPSToken(token)
                    token = ""
                }
        } header: {
            Text("Access")
        } footer: {
            if draft.isPlainHTTP {
                Text("Plain http: nothing protects the connection, and the token is not sent over it.")
            } else {
                Text("An access token limited to this one repository, if the server has such a thing. It is kept in this device's keychain.")
            }
        }
        if draft.isHTTPS {
            Section {
                Toggle("Trust the public certificate authorities", isOn: $draft.trustPublicCAs)
                if !draft.trustPublicCAs {
                    if let pin = draft.pinSHA256 {
                        LabeledContent("Pinned") {
                            Text(groupedFingerprint(pin)).font(.caption.monospaced()).multilineTextAlignment(.trailing)
                        }
                        Toggle("Also verify with public CAs", isOn: $draft.verifyChain)
                    }
                    if let offered {
                        VStack(alignment: .leading, spacing: 8) {
                            Text("The server presents:")
                            Text(groupedFingerprint(offered.sha256))
                                .font(.callout.monospaced())
                                .textSelection(.enabled)
                            Button("It matches: trust this certificate") {
                                draft.pinSHA256 = offered.sha256
                                draft.verifyChain = offered.chainValid
                                self.offered = nil
                            }
                            .buttonStyle(.borderedProminent)
                        }
                    }
                    Button(draft.pinSHA256 == nil ? "Read the server's certificate" : "Read it again") {
                        Task {
                            busy = true
                            defer { busy = false }
                            do {
                                offered = try await readCertificate(url: draft.url)
                            } catch {
                                model.errorMessage = message(for: error)
                            }
                        }
                    }
                    .disabled(busy)
                }
            } header: {
                Text("Certificate")
            } footer: {
                Text("A hosting service: trust the public authorities. A server of your own: pin its certificate, after comparing its SHA-256 with openssl x509 -fingerprint -sha256 on the server.")
            }
        }
    }
}

/// Who the store is encrypted to, and the public keys this device has for them.
private struct RecipientsSection: View {
    @Environment(AppModel.self) private var model
    @State private var armored = ""

    var body: some View {
        Section {
            ForEach(model.recipientFiles, id: \.path) { file in
                VStack(alignment: .leading, spacing: 4) {
                    Text(file.path).font(.headline)
                    ForEach(file.ids, id: \.self) { id in
                        Label(id, systemImage: file.unknown.contains(id) ? "questionmark.circle" : "checkmark.circle")
                            .foregroundStyle(file.unknown.contains(id) ? .orange : .primary)
                    }
                    Text(".gpg-id.sig: \(signatureText(file.signature))").foregroundStyle(.secondary)
                }
                .font(.caption.monospaced())
            }
            TextEditor(text: $armored)
                .font(.caption.monospaced())
                .frame(minHeight: 60)
            Button("Add this public key") {
                if model.addRecipientKey(armored) {
                    armored = ""
                    Task { await model.refresh() }
                }
            }
            .disabled(armored.isEmpty)
        } header: {
            Text("Recipients")
        } footer: {
            Text("Each .gpg-id lists who can read what is under it. To write there, this device needs the public key of every one of them: paste the missing ones (gpg --export --armor <id>). An entry is never written for fewer readers than the store says.")
        }
    }
}
