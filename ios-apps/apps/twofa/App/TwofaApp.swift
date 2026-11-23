import SwiftUI
import TwofaKit

@main
struct TwofaApp: App {
    @State private var model = AppModel(
        store: .device,
        defaults: .standard,
        clipboard: SystemClipboard(),
        authenticator: DeviceAuthenticator())

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

    /// Set when the app leaves the screen. Not on `.inactive` alone: the Face ID sheet itself
    /// makes the app inactive, and asking again each time it closes would never end.
    @State private var wasInBackground = false

    var body: some View {
        ZStack {
            if model.isLocked {
                LockedView()
            } else {
                KeysView()
            }
            if scenePhase != .active {
                // What the app switcher photographs.
                Rectangle()
                    .fill(.background)
                    .ignoresSafeArea()
                    .overlay { Image(systemName: "lock.fill").font(.largeTitle).foregroundStyle(.secondary) }
            }
        }
        .task { await model.unlock() }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .background:
                wasInBackground = true
                model.lock()
            case .active where wasInBackground:
                wasInBackground = false
                Task { await model.unlock() }
            default:
                break
            }
        }
        .alert(
            "2fa",
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

struct LockedView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 24) {
            Image(systemName: "lock.fill")
                .font(.system(size: 48))
                .foregroundStyle(.secondary)
            Button("Unlock") {
                Task { await model.unlock() }
            }
            .buttonStyle(.borderedProminent)
        }
    }
}
