import PassKit
import SwiftUI

@main
struct PassApp: App {
    @State private var model = AppModel(
        store: .shared,
        // Application Support: not shown in Files, and only ever holds encrypted entries and the git history.
        storeDirectory: URL.applicationSupportDirectory.appending(path: "password-store"),
        pasteboard: DevicePasteboard())

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
        }
    }
}

struct RootView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        @Bindable var model = model
        Group {
            switch model.stage {
            case .needsKey, .needsDropZone:
                // Until there is a key and a drop zone, the settings are the app.
                NavigationStack { SettingsView() }
            case .needsStore, .ready:
                if !model.isUnlocked {
                    LockView()
                } else if model.stage == .ready {
                    StoreView()
                } else {
                    NavigationStack { FirstSyncView() }
                }
            }
        }
        // What the app switcher shows of the app: nothing.
        .overlay {
            if scenePhase != .active {
                Rectangle().fill(.background).ignoresSafeArea()
                    .overlay { Image(systemName: "lock.fill").font(.largeTitle).foregroundStyle(.secondary) }
            }
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .background: model.didEnterBackground()
            case .active: Task { await model.refresh() }
            default: break
            }
        }
        // The lock timeout, checked while the app is in front.
        .task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                model.lockIfExpired()
            }
        }
        .alert(
            "Pass",
            isPresented: Binding(
                get: { model.errorMessage != nil },
                set: { if !$0 { model.errorMessage = nil } })
        ) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(model.errorMessage ?? "")
        }
    }
}

/// Locked: Face ID, or the passphrase. Nothing else happens without the key, not even a sync.
struct LockView: View {
    @Environment(AppModel.self) private var model
    @State private var passphrase = ""
    @State private var busy = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    if model.canUnlockWithBiometrics {
                        Button {
                            Task { await unlock { await model.unlockWithBiometrics() } }
                        } label: {
                            Label("Unlock with Face ID", systemImage: "faceid")
                        }
                    }
                    SecureField("Passphrase of the key", text: $passphrase)
                        .textContentType(.password)
                        .onSubmit { Task { await unlockWithPassphrase() } }
                    Button("Unlock") {
                        Task { await unlockWithPassphrase() }
                    }
                    .disabled(busy)
                } footer: {
                    if let info = model.keyInfo {
                        Text(info.userIds.first ?? groupedFingerprint(info.fingerprint))
                    }
                }
            }
            .navigationTitle("Locked")
            .toolbar {
                NavigationLink {
                    SettingsView()
                } label: {
                    Image(systemName: "gear")
                }
            }
            .overlay {
                if busy { ProgressView() }
            }
            .task {
                if model.canUnlockWithBiometrics {
                    await unlock { await model.unlockWithBiometrics() }
                }
            }
        }
    }

    private func unlock(_ attempt: () async -> Bool) async {
        busy = true
        defer { busy = false }
        _ = await attempt()
    }

    private func unlockWithPassphrase() async {
        let typed = passphrase
        passphrase = ""
        await unlock { await model.unlock(passphrase: typed) }
    }
}

/// Everything is set up but the store itself: sync and take what another device published, or
/// make the store here.
struct FirstSyncView: View {
    @Environment(AppModel.self) private var model
    @State private var readers: Set<String> = []
    @State private var busy = false

    var body: some View {
        Form {
            SyncSection()
            Section {
                ForEach(model.settings.deviceNames, id: \.self) { name in
                    Toggle(name == model.settings.deviceName ? "\(name) (this device)" : name, isOn: Binding(
                        get: { readers.contains(name) },
                        set: { if $0 { readers.insert(name) } else { readers.remove(name) } }))
                }
                Button("Make the store here") {
                    Task {
                        busy = true
                        defer { busy = false }
                        await model.createStore(readers: Array(readers))
                    }
                }
                .disabled(busy || readers.isEmpty)
            } header: {
                Text("Or start a new store")
            } footer: {
                Text("Only when no device has a store yet: two stores made apart do not merge. Its readers are the devices chosen; folders can have their own later.")
            }
        }
        .navigationTitle("The store")
        .toolbar {
            NavigationLink {
                SettingsView()
            } label: {
                Image(systemName: "gear")
            }
        }
        .task {
            readers = Set(model.settings.deviceNames)
            await model.refresh()
        }
    }
}
