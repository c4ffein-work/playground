import CompanionKit
import SwiftUI
import UIKit

struct PadView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase
    @FocusState private var editing: Bool

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            TextEditor(text: $model.padText)
                .font(.body.monospaced())
                .focused($editing)
                .padding(.horizontal, 8)
                .navigationTitle("Pad")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .topBarLeading) {
                        Text(model.padIsDirty ? "Sending…" : "In sync")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    ToolbarItemGroup(placement: .topBarTrailing) {
                        Button {
                            UIPasteboard.general.string = model.padText
                        } label: {
                            Label("Copy all", systemImage: "doc.on.doc")
                        }
                        .disabled(model.padText.isEmpty)
                        Button {
                            guard let pasted = UIPasteboard.general.string else { return }
                            let separator = model.padText.isEmpty || model.padText.hasSuffix("\n") ? "" : "\n"
                            model.padText += separator + pasted
                        } label: {
                            Label("Paste at the end", systemImage: "doc.on.clipboard")
                        }
                        if editing {
                            Button("Done") { editing = false }
                        }
                    }
                }
                // Other devices write the pad too: take their edits while it is on screen.
                .task(id: scenePhase) {
                    guard scenePhase == .active else { return }
                    while !Task.isCancelled {
                        await model.pullPad()
                        try? await Task.sleep(for: .seconds(2))
                    }
                }
                // Send once the typing pauses: every keystroke restarts this.
                .task(id: model.padText) {
                    try? await Task.sleep(for: .milliseconds(700))
                    guard !Task.isCancelled else { return }
                    await model.pushPad()
                }
        }
    }
}
