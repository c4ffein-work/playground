import FeedKit
import SwiftUI

/// The feeds: add, edit, remove. And the one setting.
struct FeedsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var adding = false

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(model.feeds) { feed in
                        NavigationLink {
                            FeedForm(feed: feed)
                        } label: {
                            FeedRow(feed: feed, unread: model.unreadCount(in: .feed(feed.id)), failure: model.failures[feed.id])
                        }
                    }
                    .onDelete { offsets in
                        for feed in offsets.map({ model.feeds[$0] }) {
                            model.removeFeed(feed)
                        }
                    }
                } footer: {
                    if !model.feeds.isEmpty {
                        Text("Removing a feed removes its articles and its cookies from this device.")
                    }
                }
                Section {
                    Toggle(
                        "Load images",
                        isOn: Binding(get: { model.loadImages }, set: { model.setLoadImages($0) }))
                } footer: {
                    Text("Off, an article shows the description of its images and loads nothing. On, images are loaded over https: their servers then see your address when you open an article. Scripts, frames and fonts are never loaded.")
                }
            }
            .overlay {
                if model.feeds.isEmpty {
                    ContentUnavailableView(
                        "No feeds", systemImage: "dot.radiowaves.up.forward", description: Text("Add one with +."))
                }
            }
            .navigationTitle("Feeds")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Done") { dismiss() }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        adding = true
                    } label: {
                        Label("Add", systemImage: "plus")
                    }
                }
            }
            .navigationDestination(isPresented: $adding) {
                FeedForm(feed: nil)
            }
        }
        .errorAlert(model)
    }
}

struct FeedRow: View {
    let feed: Feed
    let unread: Int
    /// Why its last refresh failed, when it did.
    let failure: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Text(feed.name)
                Spacer()
                if unread > 0 {
                    Text("\(unread)")
                        .font(.footnote.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
            }
            Text(feed.url)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
            if let failure {
                Label(failure, systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.red)
            }
        }
    }
}

/// A new feed (`feed` is `nil`) or an existing one. Nothing is saved before the feed has answered.
struct FeedForm: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    let feed: Feed?

    @State private var name = ""
    @State private var url = ""
    @State private var cookies = ""
    @State private var busy = false

    var body: some View {
        Form {
            Section {
                TextField("https://example.com/feed.xml", text: $url)
                    .keyboardType(.URL)
                    .textContentType(.URL)
                TextField("Name (the feed's own when empty)", text: $name)
            }
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()

            Section {
                SecureField("session=abc; user=42", text: $cookies)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
            } header: {
                Text("Cookies")
            } footer: {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Only for a feed behind a login: the Cookie header your browser sends to it. Kept in the keychain, and sent to this feed's server only: not to where it redirects.")
                    if cookiesWouldBeSentInClear(url: url, cookies: cookies) {
                        Label("This address is http://: the cookies will cross the network unencrypted.", systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                    }
                }
            }

            Section {
                Button {
                    Task { await save() }
                } label: {
                    HStack {
                        Text(feed == nil ? "Add" : "Save")
                        Spacer()
                        if busy {
                            ProgressView()
                        }
                    }
                }
                .disabled(busy || url.trimmingCharacters(in: .whitespaces).isEmpty)
            }

            if let feed {
                Section {
                    Button("Remove this feed", role: .destructive) {
                        model.removeFeed(feed)
                        dismiss()
                    }
                }
            }
        }
        .navigationTitle(feed?.name ?? "New feed")
        .navigationBarTitleDisplayMode(.inline)
        .onAppear {
            guard let feed, url.isEmpty else { return }
            name = feed.name
            url = feed.url
            cookies = model.cookies(of: feed)
        }
    }

    private func save() async {
        busy = true
        defer { busy = false }
        let saved: Bool
        if let feed {
            saved = await model.updateFeed(feed, name: name, url: url, cookies: cookies)
        } else {
            saved = await model.addFeed(name: name, url: url, cookies: cookies)
        }
        if saved {
            dismiss()
        }
    }
}
