import FeedKit
import SwiftUI

@main
struct FeedApp: App {
    @State private var model = AppModel(
        store: .shared,
        library: Library(directory: URL.applicationSupportDirectory.appending(path: "library")))

    var body: some Scene {
        WindowGroup {
            ArticlesView()
                .environment(model)
                .errorAlert(model)
        }
    }
}

extension View {
    /// Shows the model's last failure. A sheet covers the alerts of what is under it: it asks for its own.
    func errorAlert(_ model: AppModel) -> some View {
        alert(
            "Feed",
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
