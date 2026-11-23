import SwiftUI
import TwofaKit

/// The form a key is typed in, or reviewed in after a scan. What is valid is for the model to say.
struct AddKeyView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State var draft: KeyDraft
    /// Why the last attempt was refused: shown here, the alert of the list being under this sheet.
    @State private var problem: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Name", text: $draft.name)
                    TextField("Key", text: $draft.secret, axis: .vertical)
                        .font(.body.monospaced())
                } footer: {
                    Text("The name: letters, digits, - / and =. The key: the letters and digits the service shows beside its QR code.")
                }
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()

                Section {
                    Picker("Digits", selection: $draft.digits) {
                        ForEach([6, 7, 8], id: \.self) { Text("\($0)").tag($0) }
                    }
                    .pickerStyle(.segmented)
                    Toggle("Counter-based (HOTP)", isOn: $draft.isCounterBased)
                    if draft.isCounterBased {
                        LabeledContent("Last counter used") {
                            TextField("0", value: $draft.counter, format: .number.grouping(.never))
                                .keyboardType(.numberPad)
                                .multilineTextAlignment(.trailing)
                        }
                    }
                } footer: {
                    Text("Leave these alone unless the service says otherwise: almost all use 6 digits, based on time.")
                }

                Section {
                    PasteButton(payloadType: String.self) { strings in
                        paste(strings.first ?? "")
                    }
                } footer: {
                    Text("Pastes an otpauth:// link, in place of scanning its QR code.")
                }

                if let problem {
                    Section {
                        Text(problem).foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("Add a key")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Add") { add() }
                        .disabled(draft.name.isEmpty || draft.secret.isEmpty)
                }
            }
        }
    }

    private func add() {
        if model.add(draft) {
            dismiss()
        } else {
            takeProblem()
        }
    }

    private func paste(_ link: String) {
        if let pasted = model.draft(fromURI: link) {
            draft = pasted
            problem = nil
        } else {
            takeProblem()
        }
    }

    /// Moves the model's error here, before the list tries to show it from under this sheet.
    private func takeProblem() {
        problem = model.errorMessage
        model.errorMessage = nil
    }
}
