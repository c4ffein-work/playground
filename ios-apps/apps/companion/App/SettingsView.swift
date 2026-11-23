import CompanionKit
import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model

    @State private var draft = ServerSettings()
    @State private var secret = ""
    @State private var busy = false
    @State private var connected = false
    /// A certificate read from the server, waiting to be compared and trusted.
    @State private var offered: CertificateFingerprint?

    var body: some View {
        Form {
            Section {
                TextField("http://192.168.1.10:8080", text: $draft.url)
                    .keyboardType(.URL)
                    .textContentType(.URL)
                TextField("Client ID", text: $draft.clientId)
                SecureField("Client secret", text: $secret)
            } header: {
                Text("Server")
            } footer: {
                Text("On the server: `companion server-add-user` prints an ID and a secret for this device.")
            }
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()

            if draft.isHTTPS {
                certificateSection
            }

            Section {
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
                .disabled(busy || !draft.isComplete || secret.isEmpty)
            }

            if model.isConnected {
                Section {
                    Button("Clear downloaded files", role: .destructive) {
                        model.clearDownloads()
                    }
                } footer: {
                    Text("Only the copies on this device. Nothing is deleted on the server.")
                }
            }
        }
        .navigationTitle("Settings")
        .onAppear {
            draft = model.settings
            secret = model.secret
        }
        .onChange(of: draft) { connected = false }
        .onChange(of: secret) { connected = false }
        // A pin belongs to the server it was read from.
        .onChange(of: draft.url) { old, new in
            if !old.isEmpty && old != new {
                draft.pinSHA256 = nil
                offered = nil
            }
        }
    }

    @ViewBuilder
    private var certificateSection: some View {
        Section {
            if let pin = draft.pinSHA256 {
                LabeledContent("Pinned") {
                    Text(groupedFingerprint(pin))
                        .font(.caption.monospaced())
                        .multilineTextAlignment(.trailing)
                }
                Toggle("Also verify with public CAs", isOn: $draft.verifyChain)
            }
            if let offered {
                VStack(alignment: .leading, spacing: 8) {
                    Text("The server presents:")
                    Text(groupedFingerprint(offered.sha256))
                        .font(.callout.monospaced())
                        .textSelection(.enabled)
                    Text(offered.chainValid
                        ? "Signed by a public CA for this name."
                        : "Not verifiable by a public CA (self-signed, or another name): the pin alone will identify it.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Button("It matches: trust this certificate") {
                        draft.pinSHA256 = offered.sha256
                        draft.verifyChain = offered.chainValid
                        self.offered = nil
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
            Button(draft.pinSHA256 == nil ? "Read the server's certificate" : "Read it again") {
                Task { await readCertificate() }
            }
            .disabled(busy)
        } header: {
            Text("Certificate")
        } footer: {
            Text("Compare with what `companion fingerprint` prints on a machine that already trusts the server. Nothing is sent over https before a certificate is pinned.")
        }
    }

    private func readCertificate() async {
        busy = true
        defer { busy = false }
        do {
            offered = try await fetchCertificate(url: draft.url)
        } catch {
            model.errorMessage = message(for: error)
        }
    }

    private func connect() async {
        busy = true
        defer { busy = false }
        connected = await model.connect(settings: draft, secret: secret)
    }
}
