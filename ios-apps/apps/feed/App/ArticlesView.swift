import FeedKit
import SwiftUI

/// Every article, newest first: the whole app starts here.
struct ArticlesView: View {
    @Environment(AppModel.self) private var model

    @State private var managingFeeds = false

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            List(model.visibleArticles) { article in
                NavigationLink(value: article.id) {
                    ArticleRow(
                        article: article,
                        // One feed on screen: its name on every row says nothing.
                        feedName: model.filter == .all ? model.feed(id: article.feedID)?.name : nil,
                        isRead: model.isRead(article))
                }
                .swipeActions(edge: .leading) {
                    Button {
                        model.toggleRead(article)
                    } label: {
                        if model.isRead(article) {
                            Label("Unread", systemImage: "circle.fill")
                        } else {
                            Label("Read", systemImage: "checkmark.circle")
                        }
                    }
                    .tint(.blue)
                }
            }
            .listStyle(.plain)
            .overlay {
                if model.feeds.isEmpty {
                    ContentUnavailableView {
                        Label("No feeds", systemImage: "dot.radiowaves.up.forward")
                    } description: {
                        Text("Add the address of an RSS or Atom feed.")
                    } actions: {
                        Button("Add a feed") { managingFeeds = true }
                    }
                } else if model.visibleArticles.isEmpty {
                    ContentUnavailableView(
                        model.unreadOnly ? "Nothing unread" : "No articles", systemImage: "tray",
                        description: Text("Pull down to refresh."))
                }
            }
            .navigationTitle(model.filterName)
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(for: Article.ID.self) { id in
                ArticleView(articleID: id)
            }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button {
                        managingFeeds = true
                    } label: {
                        Label("Feeds", systemImage: model.failures.isEmpty ? "list.bullet" : "exclamationmark.triangle")
                    }
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Picker("Feed", selection: $model.filter) {
                            Text("All articles (\(model.unreadCount(in: .all)))").tag(ArticleFilter.all)
                            ForEach(model.feeds) { feed in
                                Text("\(feed.name) (\(model.unreadCount(in: .feed(feed.id))))")
                                    .tag(ArticleFilter.feed(feed.id))
                            }
                        }
                        Toggle("Unread only", isOn: $model.unreadOnly)
                        Button {
                            model.markAllRead()
                        } label: {
                            Label("Mark all as read", systemImage: "checkmark.circle")
                        }
                    } label: {
                        Label(
                            "Filter",
                            systemImage: model.filter == .all && !model.unreadOnly
                                ? "line.3.horizontal.decrease.circle" : "line.3.horizontal.decrease.circle.fill")
                    }
                }
            }
            .refreshable { await model.refresh() }
            .sheet(isPresented: $managingFeeds) {
                FeedsView()
                    .environment(model)
            }
        }
        // Once, when the app opens. What was on the phone is already on screen: this only brings what is new.
        .task { await model.refresh(quiet: true) }
    }
}

struct ArticleRow: View {
    let article: Article
    let feedName: String?
    let isRead: Bool

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Circle()
                .fill(isRead ? Color.clear : Color.accentColor)
                .frame(width: 8, height: 8)
            VStack(alignment: .leading, spacing: 3) {
                Text(article.displayTitle)
                    .font(.headline)
                    .foregroundStyle(isRead ? .secondary : .primary)
                    .lineLimit(3)
                if !article.summary.isEmpty {
                    Text(article.summary)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
                Text(details)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }
        }
    }

    private var details: String {
        var parts: [String] = []
        if let feedName {
            parts.append(feedName)
        }
        parts.append(article.sortDate.formatted(.relative(presentation: .named)))
        return parts.joined(separator: " · ")
    }
}
