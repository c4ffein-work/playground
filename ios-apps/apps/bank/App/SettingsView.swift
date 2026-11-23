import BankKit
import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model

    @State private var draft = Credentials()
    @State private var busy = false
    @State private var connected = false

    var body: some View {
        Form {
            // First: there is no connection, and nothing to send a key to, without a pinned certificate.
            ForEach(PinTarget.allCases) { target in
                PinSection(target: target)
            }

            Section {
                TextField("Organization slug", text: $draft.organizationSlug)
                SecureField("Secret key", text: $draft.secretKey)
                Button {
                    Task { await connect() }
                } label: {
                    HStack {
                        Text("Save and connect")
                        Spacer()
                        if busy {
                            ProgressView()
                        } else if connected {
                            Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                        }
                    }
                }
                .disabled(busy || !draft.isComplete || !model.settings.isComplete)
            } header: {
                Text("API key")
            } footer: {
                Text("In Qonto: Settings, Integrations, API key. Kept in the keychain of this device, and only sent to the Qonto API, over a pinned connection.")
            }
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()

            if model.isConnected {
                Section {
                    Button("Clear downloaded attachments") {
                        model.clearDownloads()
                    }
                    Button("Sign out", role: .destructive) {
                        model.signOut()
                        draft = Credentials()
                    }
                } footer: {
                    Text("Signing out removes the key and the downloaded attachments from this device. Nothing is deleted at the bank.")
                }
            }
        }
        .navigationTitle("Settings")
        .onAppear { draft = model.credentials }
        .onChange(of: draft) { connected = false }
    }

    private func connect() async {
        busy = true
        defer { busy = false }
        connected = await model.connect(draft)
    }
}

/// The pins of one host: the ones that are trusted, and the two ways to add one (see BankKit/Pins.swift).
struct PinSection: View {
    @Environment(AppModel.self) private var model
    let target: PinTarget

    @State private var busy = false
    /// A certificate read from the host, waiting to be compared and pinned.
    @State private var offered: PinOffer?
    @State private var pasting = false
    @State private var pasted = ""

    var body: some View {
        Section {
            if let problem = model.pinProblem, problem.target == target {
                VStack(alignment: .leading, spacing: 6) {
                    Label("\(problem.host) serves a certificate that is not pinned", systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.orange)
                    Text(groupedFingerprint(problem.served))
                        .font(.caption.monospaced())
                    Text("Certificates are renewed every few months. Nothing is sent until the new one is pinned: read it below, and compare.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            if target == .attachments {
                TextField("Host", text: Binding(
                    get: { model.settings.attachmentsHost },
                    set: { model.setAttachmentsHost($0) }))
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
            }
            ForEach(model.settings.pins(for: target), id: \.self) { pin in
                Text(groupedFingerprint(pin))
                    .font(.caption.monospaced())
                    .swipeActions {
                        Button("Remove", role: .destructive) {
                            model.removePin(pin, for: target)
                        }
                    }
            }
            if let offered {
                offer(offered)
            }
            Button(busy ? "Reading…" : "Read the served certificate") {
                Task { await read() }
            }
            .disabled(busy)
            Button("Paste pins") {
                pasted = ""
                pasting = true
            }
            .sheet(isPresented: $pasting) { pasteSheet }
        } header: {
            Text(target.title)
        } footer: {
            Text(footer)
        }
    }

    private var footer: String {
        let host = model.settings.host(for: target)
        switch target {
        case .api:
            return "SHA-256 of the certificates accepted from \(host). Compare with what `tls-pin-get \(host):443` prints on a computer you trust. Swipe a pin to remove it."
        case .attachments:
            return "The same for where attachments are downloaded from. Without a pin, receipts cannot be opened, and a receipt cannot be sent to a transaction that already has one."
        }
    }

    @ViewBuilder
    private func offer(_ offered: PinOffer) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("\(offered.host) presents:")
            Text(groupedFingerprint(offered.sha256))
                .font(.callout.monospaced())
                .textSelection(.enabled)
            if offered.alreadyPinned {
                Label("Already pinned", systemImage: "checkmark.seal")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else if offered.chainValid {
                Text("Signed by a public CA for this name. That alone is what any website has: compare it with a second source before trusting it.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button("It matches: pin this certificate") {
                    if model.trust(offered) {
                        self.offered = nil
                    }
                }
                .buttonStyle(.borderedProminent)
            } else {
                Label("Not signed by a public CA for this name: this is not the bank. It cannot be pinned. Try another network.", systemImage: "xmark.octagon.fill")
                    .font(.caption)
                    .foregroundStyle(.red)
            }
        }
    }

    private var pasteSheet: some View {
        NavigationStack {
            Form {
                Section {
                    TextEditor(text: $pasted)
                        .font(.caption.monospaced())
                        .frame(minHeight: 160)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } footer: {
                    // An alert of the root view does not show over a sheet: what was wrong is said here.
                    Text(model.errorMessage ?? "One SHA-256 per line: what `tls-pin-get` prints. All of them are added, or none.")
                }
            }
            .navigationTitle(target.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { pasting = false }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add") {
                        if model.addPins(pasted: pasted, for: target) {
                            pasting = false
                        }
                    }
                    .disabled(pasted.isEmpty)
                }
            }
        }
    }

    private func read() async {
        busy = true
        defer { busy = false }
        offered = await model.readCertificate(for: target)
    }
}
