import CompanionKit
import SwiftUI

@main
struct CompanionApp: App {
    @State private var model = AppModel(
        store: .shared,
        downloadDirectory: URL.cachesDirectory.appending(path: "downloads"))

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
        }
    }
}

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            if model.isConnected {
                TabView {
                    FilesView()
                        .tabItem { Label("Files", systemImage: "folder") }
                    PadView()
                        .tabItem { Label("Pad", systemImage: "square.and.pencil") }
                    LiveView()
                        .tabItem { Label("Live", systemImage: "play.rectangle") }
                    NavigationStack { SettingsView() }
                        .tabItem { Label("Settings", systemImage: "gear") }
                }
            } else {
                NavigationStack { SettingsView() }
            }
        }
        .alert(
            "Companion",
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
