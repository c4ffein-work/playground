import FeedCore
import Foundation

/// An article of a feed, as it is kept on the phone.
public struct Article: Codable, Equatable, Identifiable, Sendable {
    /// What read/unread is kept by: feed.py's article id (the URL-safe base64 of the link).
    /// The same article in two feeds has one key: read in one, read in the other.
    public let key: String
    public let feedID: UUID
    /// One line, sanitized by the core. May be empty.
    public var title: String
    public var link: String
    /// As written in the feed.
    public var date: String
    /// `date` as Unix seconds, when the core could read it.
    public var timestamp: Int64?
    public var author: String
    /// Plain text, for the list.
    public var summary: String
    /// HTML as the feed wrote it: never shown but through the core's `articlePage`.
    public var content: String
    /// When this phone first saw it.
    public let firstSeen: Date

    public var id: String { "\(feedID.uuidString)/\(key)" }

    public var displayTitle: String { title.isEmpty ? "(no title)" : title }

    /// What the list is sorted by: the article's own date, or the day it was first seen when it has none.
    /// (feed.py puts those last, which on a phone is where nobody looks.)
    public var sortDate: Date {
        timestamp.map { Date(timeIntervalSince1970: TimeInterval($0)) } ?? firstSeen
    }

    /// The page of the article, when the feed gave an http(s) link: the only kind that is ever opened.
    public var webURL: URL? { openableURL(link) }

    init(_ entry: Entry, feedID: UUID, firstSeen: Date) {
        key = entry.id
        self.feedID = feedID
        title = entry.title
        link = entry.link
        date = entry.date
        timestamp = entry.timestamp
        author = entry.author
        summary = entry.summary
        content = entry.content
        self.firstSeen = firstSeen
    }
}

/// A link is what a feed says it is: only http and https are handed to the system.
/// Anything else (`javascript:`, `file:`, `tel:`, another app's scheme) is not.
public func openableURL(_ link: String) -> URL? {
    guard let url = URL(string: link.trimmingCharacters(in: .whitespacesAndNewlines)),
          let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https",
          url.host?.isEmpty == false
    else { return nil }
    return url
}

public func openableURL(_ url: URL) -> URL? {
    openableURL(url.absoluteString)
}

/// Newest first; articles of the same instant stay in the order they were given.
func newestFirst(_ articles: [Article]) -> [Article] {
    articles.enumerated()
        .sorted { ($0.element.sortDate, $1.offset) > ($1.element.sortDate, $0.offset) }
        .map(\.element)
}

/// The articles and what is read, on disk: the app opens on them before (or without) any network.
///
///     <directory>/articles/<feed id>.json    the articles of one feed
///     <directory>/read.json                  the keys of the read articles
///
/// Every file is written whole, beside itself, then renamed: it is there complete or not at all.
/// A file that cannot be read is treated as absent: a cache is fetched again, it is not worth an error.
public struct Library: Sendable {
    /// How many articles are kept for one feed. feed.py keeps every article for ever; a phone does not.
    public static let articlesPerFeed = 500

    private let directory: URL

    public init(directory: URL) {
        self.directory = directory
    }

    private var articlesDirectory: URL { directory.appendingPathComponent("articles") }
    private var readFile: URL { directory.appendingPathComponent("read.json") }

    private func file(for feed: UUID) -> URL {
        articlesDirectory.appendingPathComponent("\(feed.uuidString).json")
    }

    private func write<T: Encodable>(_ value: T, to file: URL) throws {
        try FileManager.default.createDirectory(
            at: file.deletingLastPathComponent(), withIntermediateDirectories: true)
        try JSONEncoder().encode(value).write(to: file, options: .atomic)
    }

    private func read<T: Decodable>(_ type: T.Type, from file: URL) -> T? {
        guard let data = try? Data(contentsOf: file) else { return nil }
        return try? JSONDecoder().decode(type, from: data)
    }

    public func articles(of feed: UUID) -> [Article] {
        read([Article].self, from: file(for: feed)) ?? []
    }

    public func save(_ articles: [Article], of feed: UUID) throws {
        try write(articles, to: file(for: feed))
    }

    public func remove(feed: UUID) {
        try? FileManager.default.removeItem(at: file(for: feed))
    }

    public func readKeys() -> Set<String> {
        read(Set<String>.self, from: readFile) ?? []
    }

    public func save(read keys: Set<String>) throws {
        try write(keys.sorted(), to: readFile)
    }

    /// The articles to keep for a feed once it has been fetched, newest first.
    /// - What the feed has now is kept, as it is now, with the day it was first seen.
    /// - What it had and has no more is kept too (as feed.py does), newest first, while there is room.
    public static func merge(
        cached: [Article], fetched: [Entry], feedID: UUID, now: Date, limit: Int = articlesPerFeed
    ) -> [Article] {
        let firstSeen = Dictionary(cached.map { ($0.key, $0.firstSeen) }, uniquingKeysWith: { first, _ in first })
        let current = fetched.map { Article($0, feedID: feedID, firstSeen: firstSeen[$0.id] ?? now) }
        let currentKeys = Set(current.map(\.key))
        let gone = newestFirst(cached.filter { !currentKeys.contains($0.key) })
        return newestFirst(current + gone.prefix(max(0, limit - current.count)))
    }
}
