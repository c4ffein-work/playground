import CompanionKit
import QuickLook
import SwiftUI

/// What is being presented to every client (`companion set-preview`, or "Present" on a file):
/// this device follows it, and opens each new file as it is presented.
struct LiveView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase

    @AppStorage("live-follow") private var follow = true
    @State private var opened: URL?

    var body: some View {
        NavigationStack {
            Group {
                if let name = model.preview?.filename {
                    VStack(spacing: 16) {
                        Image(systemName: "play.rectangle.fill")
                            .font(.system(size: 56))
                            .foregroundStyle(.tint)
                        Text(name)
                            .font(.title3)
                            .multilineTextAlignment(.center)
                        Button("Open") {
                            Task { await open() }
                        }
                        .buttonStyle(.borderedProminent)
                    }
                    .padding()
                } else {
                    ContentUnavailableView(
                        "Nothing presented", systemImage: "play.slash",
                        description: Text("Swipe a file and choose Present, on any device."))
                }
            }
            .navigationTitle("Live")
            .toolbar {
                Toggle(isOn: $follow) {
                    Label("Open what is presented", systemImage: "eye")
                }
            }
            .quickLookPreview($opened)
            .task(id: scenePhase) {
                guard scenePhase == .active else { return }
                while !Task.isCancelled {
                    if await model.refreshPreview(), follow {
                        await open()
                    }
                    try? await Task.sleep(for: .seconds(2))
                }
            }
        }
    }

    private func open() async {
        guard let file = await model.presentedFile() else { return }
        opened = await model.localCopy(of: file)
    }
}
