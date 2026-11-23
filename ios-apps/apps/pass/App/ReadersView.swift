import PassKit
import SwiftUI

/// Who reads a folder: the devices its own `.gpg-id` names, chosen among the devices of the
/// sync, or none of its own, and then those of the folder above.
struct ReadersView: View {
    /// `""` is the top of the store.
    let folder: String
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var access: Access?
    @State private var hasOwn = false
    @State private var chosen: Set<String> = []
    @State private var saved: Set<String>?
    @State private var busy = false

    private var isRoot: Bool { folder.isEmpty }
    private var label: String { isRoot ? "/" : folder + "/" }
    private var staleHere: [String] {
        model.stale.filter { isRoot || $0.hasPrefix(folder + "/") }
    }

    var body: some View {
        Form {
            if let access {
                Section {
                    LabeledContent("Read by", value: describe(access.readers))
                    LabeledContent("Named in") { Text(access.gpgId).font(.callout.monospaced()) }
                    if !access.listed {
                        Label("Not by this device", systemImage: "lock.slash").foregroundStyle(.secondary)
                    }
                }
            }
            Section {
                if !isRoot {
                    Toggle("Readers of its own", isOn: $hasOwn)
                }
                if hasOwn || isRoot {
                    ForEach(model.settings.deviceNames, id: \.self) { name in
                        Toggle(name == model.settings.deviceName ? "\(name) (this device)" : name, isOn: Binding(
                            get: { chosen.contains(name) },
                            set: { if $0 { chosen.insert(name) } else { chosen.remove(name) } }))
                    }
                }
            } header: {
                Text("Readers of \(label)")
            } footer: {
                Text(footer)
            }
            if !staleHere.isEmpty {
                Section {
                    ForEach(staleHere, id: \.self) { name in
                        Label(name, systemImage: "lock.rotation")
                    }
                } header: {
                    Text("Not encrypted for these readers yet")
                } footer: {
                    Text("A device that reads them encrypts them again when it takes this change. Until then they are read by whoever they were encrypted to.")
                }
            }
            Section {
                Button("Save") {
                    Task {
                        busy = true
                        defer { busy = false }
                        let readers = hasOwn || isRoot ? Array(chosen) : []
                        if await model.setReaders(ofFolder: folder, to: readers) {
                            await load()
                        }
                    }
                }
                .disabled(busy || !changed || ((hasOwn || isRoot) && chosen.isEmpty))
            }
        }
        .navigationTitle("Readers")
        .navigationBarTitleDisplayMode(.inline)
        .overlay {
            if busy { ProgressView() }
        }
        .task { await load() }
        .onChange(of: model.isUnlocked) { _, unlocked in
            if !unlocked { dismiss() }
        }
    }

    private var changed: Bool {
        (hasOwn || isRoot ? chosen : nil) != saved
    }

    private var footer: String {
        if access?.listed == false {
            return "This device does not read here. Naming it is asking: it reads once a device that does has taken the change and encrypted the entries again."
        }
        return "What this device reads here is encrypted again for the readers chosen, and published. A device taken off the list keeps what it already received: change the passwords that matter."
    }

    private func load() async {
        access = await model.access(toFolder: folder)
        let own = await model.ownReaders(ofFolder: folder)
        hasOwn = own != nil
        chosen = Set(own ?? access?.readers.compactMap(\.device) ?? [])
        saved = own.map(Set.init)
    }
}
