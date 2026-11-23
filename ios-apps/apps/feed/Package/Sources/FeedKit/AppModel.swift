import FeedCore
import Foundation
import Observation

/// Which articles the list shows.
public enum ArticleFilter: Hashable, Sendable {
    case all
    case feed(UUID)
}

/// Everything the views show, and everything they can ask for. No view fetches, parses or stores anything itself.
@MainActor
@Observable
public final class AppModel {
    public private(set) var feeds: [Feed]
    /// Every article on the phone, newest first.
    public private(set) var articles: [Article] = []
    /// The keys of the read articles.
    public private(set) var readKeys: Set<String>

    public var filter: ArticleFilter = .all
    public var unreadOnly = false

    public private(set) var isRefreshing = false
    /// Feed id → why its last refresh failed. Its articles stay: the cache is not touched by a failure.
    public private(set) var failures: [UUID: String] = [:]
    /// Whether an article loads its images from the network (https only). Off by default:
    /// an image tells its server who reads what, and when.
    public private(set) var loadImages: Bool

    /// The last failure, for an alert. Set to `nil` to dismiss.
    public var errorMessage: String?

    private let store: FeedStore
    private let library: Library
    private let now: @Sendable () -> Date
    private var byFeed: [UUID: [Article]] = [:]

    /// Reads the feeds and the cache from disk, and nothing from the network: the app opens on what it had.
    public init(store: FeedStore, library: Library, now: @escaping @Sendable () -> Date = { Date() }) {
        self.store = store
        self.library = library
        self.now = now
        feeds = store.loadFeeds()
        readKeys = library.readKeys()
        loadImages = store.loadImages
        for feed in feeds {
            byFeed[feed.id] = library.articles(of: feed.id)
        }
        rebuild()
    }

    private func rebuild() {
        articles = newestFirst(feeds.flatMap { byFeed[$0.id] ?? [] })
    }

    private func fail(_ error: Error) {
        errorMessage = message(for: error)
    }

    // MARK: What the list shows

    public func articles(in filter: ArticleFilter) -> [Article] {
        switch filter {
        case .all: return articles
        case .feed(let id): return articles.filter { $0.feedID == id }
        }
    }

    /// The articles of the current filter, newest first, without the read ones when `unreadOnly` is set.
    public var visibleArticles: [Article] {
        let shown = articles(in: filter)
        return unreadOnly ? shown.filter { !isRead($0) } : shown
    }

    public func isRead(_ article: Article) -> Bool {
        readKeys.contains(article.key)
    }

    public func unreadCount(in filter: ArticleFilter) -> Int {
        articles(in: filter).reduce(0) { $0 + (isRead($1) ? 0 : 1) }
    }

    public func article(id: Article.ID) -> Article? {
        articles.first { $0.id == id }
    }

    public func feed(id: UUID) -> Feed? {
        feeds.first { $0.id == id }
    }

    /// The title of the list: the name of the feed it is filtered on.
    public var filterName: String {
        if case .feed(let id) = filter, let feed = feed(id: id) {
            return feed.name
        }
        return "All articles"
    }

    // MARK: Read and unread

    public func setRead(_ read: Bool, _ article: Article) {
        setRead(read, keys: [article.key])
    }

    public func toggleRead(_ article: Article) {
        setRead(!isRead(article), article)
    }

    /// Marks everything the list shows as read.
    public func markAllRead() {
        setRead(true, keys: visibleArticles.map(\.key))
    }

    private func setRead(_ read: Bool, keys: [String]) {
        let before = readKeys
        if read {
            readKeys.formUnion(keys)
        } else {
            readKeys.subtract(keys)
        }
        if readKeys != before {
            saveRead()
        }
    }

    private func saveRead() {
        do {
            try library.save(read: readKeys)
        } catch {
            fail(error)
        }
    }

    /// Forgets the keys of the articles that are no longer on the phone.
    private func pruneRead() {
        let known = Set(articles.map(\.key))
        if !readKeys.isSubset(of: known) {
            readKeys.formIntersection(known)
            saveRead()
        }
    }

    // MARK: Refreshing

    /// Fetches every feed, all at once, and merges what they have into the cache.
    /// A feed that fails keeps its articles, and says why in `failures`.
    /// `quiet`: for the refresh on launch, which must not raise an alert when there is no network.
    public func refresh(quiet: Bool = false) async {
        guard !isRefreshing else { return }
        isRefreshing = true
        defer { isRefreshing = false }
        let targets = feeds.map { (feed: $0, cookies: store.cookies(for: $0.id)) }
        await withTaskGroup(of: (Feed, Result<ParsedFeed, Error>).self) { group in
            for target in targets {
                group.addTask {
                    do {
                        return (target.feed, .success(try await fetch(url: target.feed.url, cookies: target.cookies)))
                    } catch {
                        return (target.feed, .failure(error))
                    }
                }
            }
            for await (feed, result) in group {
                // Removed or pointed elsewhere while it was being fetched: this answer is about nothing.
                guard self.feed(id: feed.id)?.url == feed.url else { continue }
                switch result {
                case .success(let parsed):
                    failures[feed.id] = nil
                    keep(parsed.entries, of: feed.id)
                case .failure(let error):
                    failures[feed.id] = message(for: error)
                }
            }
        }
        pruneRead()
        if !quiet, let summary = failureSummary {
            errorMessage = summary
        }
    }

    /// One sentence for the feeds that failed, in the order of the list.
    private var failureSummary: String? {
        let failed = feeds.compactMap { feed in failures[feed.id].map { "\(feed.name): \($0)" } }
        return failed.isEmpty ? nil : failed.joined(separator: "\n")
    }

    /// Merges what a feed has now into its cache, in memory and on disk.
    private func keep(_ entries: [Entry], of feed: UUID) {
        let merged = Library.merge(cached: byFeed[feed] ?? [], fetched: entries, feedID: feed, now: now())
        byFeed[feed] = merged
        rebuild()
        do {
            try library.save(merged, of: feed)
        } catch {
            fail(error)
        }
    }

    // MARK: Feeds

    /// The cookies of a feed, for its form.
    public func cookies(of feed: Feed) -> String {
        store.cookies(for: feed.id) ?? ""
    }

    /// Fetches the feed and only then adds it: what is in the list is known to work.
    /// With no name, the feed's own title is used. Returns whether it worked; the reason is in `errorMessage` when it did not.
    @discardableResult
    public func addFeed(name: String, url: String, cookies: String) async -> Bool {
        do {
            let url = try checkFeedUrl(url: url)
            if let known = feeds.first(where: { $0.url == url }) {
                throw FeedKitError.alreadyFollowed(known.name)
            }
            let parsed = try await fetch(url: url, cookies: cookies)
            let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
            let feed = Feed(name: name.isEmpty ? defaultName(title: parsed.title, url: url) : name, url: url)
            try store.setCookies(cookies, for: feed.id)
            try store.save(feeds + [feed])
            feeds.append(feed)
            keep(parsed.entries, of: feed.id)
            return true
        } catch {
            fail(error)
            return false
        }
    }

    /// Changes the name, the URL or the cookies of a feed. A new URL or new cookies are tried first,
    /// as in `addFeed`; a feed that moved starts again from what is at its new URL.
    @discardableResult
    public func updateFeed(_ feed: Feed, name: String, url: String, cookies: String) async -> Bool {
        do {
            let url = try checkFeedUrl(url: url)
            if let known = feeds.first(where: { $0.url == url && $0.id != feed.id }) {
                throw FeedKitError.alreadyFollowed(known.name)
            }
            let moved = url != feed.url
            let cookies = cookies.trimmingCharacters(in: .whitespacesAndNewlines)
            var parsed: ParsedFeed?
            if moved || cookies != self.cookies(of: feed) {
                parsed = try await fetch(url: url, cookies: cookies)
            }
            guard let index = feeds.firstIndex(where: { $0.id == feed.id }) else { return false }
            let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
            var updated = feeds
            updated[index].name = name.isEmpty ? defaultName(title: parsed?.title ?? "", url: url) : name
            updated[index].url = url
            try store.setCookies(cookies, for: feed.id)
            try store.save(updated)
            feeds = updated
            if moved {
                byFeed[feed.id] = []
            }
            if let parsed {
                failures[feed.id] = nil
                keep(parsed.entries, of: feed.id)
                pruneRead()
            }
            return true
        } catch {
            fail(error)
            return false
        }
    }

    /// Removes a feed with its cookies and its articles.
    public func removeFeed(_ feed: Feed) {
        let remaining = feeds.filter { $0.id != feed.id }
        do {
            try store.save(remaining)
            try store.setCookies(nil, for: feed.id)
        } catch {
            fail(error)
            return
        }
        feeds = remaining
        byFeed[feed.id] = nil
        failures[feed.id] = nil
        library.remove(feed: feed.id)
        if filter == .feed(feed.id) {
            filter = .all
        }
        rebuild()
        pruneRead()
    }

    // MARK: Reading

    public func setLoadImages(_ load: Bool) {
        store.setLoadImages(load)
        loadImages = load
    }

    /// Under the title of an article: its feed, its author, its date.
    public func byline(for article: Article) -> String {
        var parts: [String] = []
        if let feed = feed(id: article.feedID) {
            parts.append(feed.name)
        }
        if !article.author.isEmpty {
            parts.append(article.author)
        }
        if article.timestamp != nil {
            let formatter = DateFormatter()
            formatter.dateStyle = .medium
            formatter.timeStyle = .short
            parts.append(formatter.string(from: article.sortDate))
        } else if !article.date.isEmpty {
            parts.append(article.date)
        }
        return parts.joined(separator: " · ")
    }

    /// The HTML document to show for an article. The article's own HTML never gets there as it is:
    /// the core rewrites it from an allowlist, in a page that lets nothing load but, when
    /// `loadImages` is set, images over https. The web view that shows it runs no JavaScript.
    public func page(for article: Article) -> String {
        articlePage(
            title: article.displayTitle, byline: byline(for: article), content: article.content,
            link: article.link, images: loadImages)
    }
}
