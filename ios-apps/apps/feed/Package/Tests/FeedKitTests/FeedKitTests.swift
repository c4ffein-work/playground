import FeedCore
import Foundation
import XCTest
@testable import FeedKit

/// A store and a library of its own for each test, removed after it.
class FeedKitTestCase: XCTestCase {
    var suite: String!
    var defaults: UserDefaults!
    var secrets: InMemorySecretStore!
    var store: FeedStore!
    var directory: URL!
    var library: Library!

    override func setUp() {
        suite = "feed-tests-\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suite)!
        secrets = InMemorySecretStore()
        store = FeedStore(defaults: defaults, secrets: secrets)
        directory = FileManager.default.temporaryDirectory.appendingPathComponent("feed-kit-\(UUID().uuidString)")
        library = Library(directory: directory)
    }

    override func tearDown() {
        defaults?.removePersistentDomain(forName: suite)
        try? FileManager.default.removeItem(at: directory)
    }
}

func entry(_ title: String, day: Int? = nil, link: String? = nil, content: String = "") -> Entry {
    let link = link ?? "https://ex.com/\(title)"
    return Entry(
        id: Data(link.utf8).base64EncodedString(), title: title, link: link,
        date: day.map { "2024-01-\($0)" } ?? "", timestamp: day.map { 1_704_067_200 + Int64($0 - 1) * 86_400 },
        author: "", summary: "", content: content)
}

final class StoreTests: FeedKitTestCase {
    func testFeedsRoundTrip() throws {
        XCTAssertEqual(store.loadFeeds(), [])
        let feeds = [Feed(name: "One", url: "https://one.example/feed"), Feed(name: "Two", url: "http://two.example/rss")]
        try store.save(feeds)
        XCTAssertEqual(store.loadFeeds(), feeds)
        XCTAssertEqual(FeedStore(defaults: defaults, secrets: secrets).loadFeeds(), feeds)
    }

    func testCookiesGoToTheSecretStoreAndNowhereElse() throws {
        let feed = Feed(name: "Forum", url: "https://forum.example/feed.xml")
        try store.save([feed])
        XCTAssertNil(store.cookies(for: feed.id))

        try store.setCookies(" session_id=abc123\n", for: feed.id)
        XCTAssertEqual(store.cookies(for: feed.id), "session_id=abc123")
        XCTAssertEqual(secrets.read("cookies-\(feed.uuidString)"), "session_id=abc123")
        // Nothing of them in UserDefaults, which is a plain file in the app's container (and in its backups).
        for (key, value) in defaults.persistentDomain(forName: suite) ?? [:] {
            let text = (value as? Data).map { String(decoding: $0, as: UTF8.self) } ?? "\(value)"
            XCTAssertFalse(text.contains("abc123"), key)
        }
        XCTAssertNil(store.cookies(for: UUID()), "one feed's cookies are not another's")

        try store.setCookies("  ", for: feed.id)
        XCTAssertNil(store.cookies(for: feed.id))
        XCTAssertTrue(secrets.isEmpty)
    }

    func testImagesAreOffUntilAskedFor() {
        XCTAssertFalse(store.loadImages)
        store.setLoadImages(true)
        XCTAssertTrue(FeedStore(defaults: defaults, secrets: secrets).loadImages)
    }

    func testCookiesOverPlainHTTPAreToldApart() {
        XCTAssertTrue(cookiesWouldBeSentInClear(url: " HTTP://forum.example/feed", cookies: "a=b"))
        XCTAssertFalse(cookiesWouldBeSentInClear(url: "https://forum.example/feed", cookies: "a=b"))
        XCTAssertFalse(cookiesWouldBeSentInClear(url: "http://forum.example/feed", cookies: "  "))
    }

    func testOnlyWebLinksAreOpened() {
        XCTAssertEqual(openableURL(" https://ex.com/a?b=c ")?.absoluteString, "https://ex.com/a?b=c")
        XCTAssertEqual(openableURL("HTTP://ex.com")?.absoluteString, "HTTP://ex.com")
        for link in [
            "", "javascript:alert(1)", "file:///etc/passwd", "tel:112", "sms:112", "itms-apps://x", "myapp://do/it",
            "data:text/html,x", "/relative", "ex.com/a", "https://", "about:blank",
        ] {
            XCTAssertNil(openableURL(link), link)
        }
        XCTAssertNil(openableURL(URL(string: "about:blank")!))
        XCTAssertNotNil(openableURL(URL(string: "https://ex.com/")!))
    }
}

extension Feed {
    var uuidString: String { id.uuidString }
}

final class LibraryTests: FeedKitTestCase {
    let feedID = UUID()
    let monday = Date(timeIntervalSince1970: 1_700_000_000)
    lazy var tuesday = monday.addingTimeInterval(86_400)

    func titles(_ articles: [Article]) -> [String] {
        articles.map(\.title)
    }

    func testMergeKeepsWhatTheFeedDropped() {
        // feed.py's test_dropped_article_persists_in_cache and test_new_article_added_after_disappearance
        let first = Library.merge(cached: [], fetched: [entry("a", day: 1), entry("b", day: 2)], feedID: feedID, now: monday)
        XCTAssertEqual(titles(first), ["b", "a"], "newest first")
        XCTAssertTrue(first.allSatisfy { $0.firstSeen == monday && $0.feedID == feedID })

        let second = Library.merge(cached: first, fetched: [entry("b", day: 2), entry("c", day: 3)], feedID: feedID, now: tuesday)
        XCTAssertEqual(titles(second), ["c", "b", "a"])
        // feed.py's test_resave_via_fetch_preserves_first_seen_at
        XCTAssertEqual(second.map(\.firstSeen), [tuesday, monday, monday])
    }

    func testMergeTakesTheFeedsCorrections() {
        let first = Library.merge(cached: [], fetched: [entry("tpyo", day: 1, link: "https://ex.com/1")], feedID: feedID, now: monday)
        let fixed = entry("typo", day: 1, link: "https://ex.com/1", content: "<p>fixed</p>")
        let second = Library.merge(cached: first, fetched: [fixed], feedID: feedID, now: tuesday)
        XCTAssertEqual(second.count, 1)
        XCTAssertEqual([second[0].title, second[0].content], ["typo", "<p>fixed</p>"])
        XCTAssertEqual(second[0].firstSeen, monday)
        XCTAssertEqual(second[0].key, first[0].key)
    }

    func testMergeIsBounded() {
        let old = (1...6).map { (day: Int) in entry("old\(day)", day: day) }
        let cached = Library.merge(cached: [], fetched: old, feedID: feedID, now: monday)
        let merged = Library.merge(
            cached: cached, fetched: [entry("new1", day: 10), entry("new2", day: 11)], feedID: feedID, now: tuesday, limit: 5)
        XCTAssertEqual(titles(merged), ["new2", "new1", "old6", "old5", "old4"], "the oldest of what the feed dropped goes first")
        // What the feed has now is never dropped, whatever the limit.
        let all = Library.merge(cached: cached, fetched: old, feedID: feedID, now: tuesday, limit: 2)
        XCTAssertEqual(all.count, 6)
    }

    func testArticlesWithNoDateAreSortedByWhenTheyWereSeen() {
        let first = Library.merge(cached: [], fetched: [entry("x"), entry("y")], feedID: feedID, now: monday)
        XCTAssertEqual(titles(first), ["x", "y"], "the order of the feed")
        let dated = entry("dated", day: 1) // 2024-01-01: after `monday` (2023-11-14)
        let second = Library.merge(cached: first, fetched: [entry("z"), dated, entry("x"), entry("y")], feedID: feedID, now: tuesday)
        XCTAssertEqual(titles(second), ["dated", "z", "x", "y"])
        XCTAssertEqual(second[1].sortDate, tuesday)
    }

    func testRoundTripOnDisk() throws {
        XCTAssertEqual(library.articles(of: feedID), [])
        XCTAssertEqual(library.readKeys(), [])

        let articles = Library.merge(
            cached: [], fetched: [entry("a", day: 1, content: "<p>é 🎉</p>"), entry("b")], feedID: feedID, now: monday)
        try library.save(articles, of: feedID)
        try library.save(read: [articles[0].key])

        let reopened = Library(directory: directory)
        XCTAssertEqual(reopened.articles(of: feedID), articles)
        XCTAssertEqual(reopened.readKeys(), [articles[0].key])
        XCTAssertEqual(reopened.articles(of: UUID()), [], "one file per feed")

        // No temporary file is left beside the real ones.
        let files = try FileManager.default.contentsOfDirectory(atPath: directory.appendingPathComponent("articles").path)
        XCTAssertEqual(files, ["\(feedID.uuidString).json"])

        reopened.remove(feed: feedID)
        XCTAssertEqual(library.articles(of: feedID), [])
        reopened.remove(feed: feedID) // twice is fine
    }

    func testAFileThatCannotBeReadIsNoArticle() throws {
        try library.save(Library.merge(cached: [], fetched: [entry("a")], feedID: feedID, now: monday), of: feedID)
        try library.save(read: ["k"])
        try Data("{ not json".utf8).write(to: directory.appendingPathComponent("articles/\(feedID.uuidString).json"))
        try Data().write(to: directory.appendingPathComponent("read.json"))
        XCTAssertEqual(library.articles(of: feedID), [])
        XCTAssertEqual(library.readKeys(), [])
    }

    func testTheSameArticleInTwoFeedsIsTwoRowsAndOneKey() {
        let other = UUID()
        let here = Library.merge(cached: [], fetched: [entry("a", day: 1)], feedID: feedID, now: monday)[0]
        let there = Library.merge(cached: [], fetched: [entry("a", day: 1)], feedID: other, now: monday)[0]
        XCTAssertNotEqual(here.id, there.id)
        XCTAssertEqual(here.key, there.key)
    }
}

/// A clock a test moves by hand.
final class Clock: @unchecked Sendable {
    private let lock = NSLock()
    private var date = Date(timeIntervalSince1970: 1_700_000_000)

    var now: Date {
        lock.lock()
        defer { lock.unlock() }
        return date
    }

    func advance(_ seconds: TimeInterval) {
        lock.lock()
        date += seconds
        lock.unlock()
    }
}

/// Swift → UniFFI → Rust → the fixture server. `apps/feed/swift-test.sh` starts it and sets FEED_E2E_URL.
@MainActor
final class EndToEndTests: FeedKitTestCase {
    private var base = ""
    private let token = UUID().uuidString
    private let clock = Clock()

    override func setUp() async throws {
        guard let url = ProcessInfo.processInfo.environment["FEED_E2E_URL"] else {
            throw XCTSkip("no fixture server: run apps/feed/swift-test.sh")
        }
        base = url
    }

    private func model() -> AppModel {
        let clock = clock
        return AppModel(store: store, library: library, now: { clock.now })
    }

    /// The URL of a feed of this test.
    private func feedURL(_ name: String = "main") -> String {
        "\(base)/feed/\(token)-\(name).xml"
    }

    /// Says what that feed holds from now on: "title:day,title:day", or a status to fail with.
    private func serve(_ items: String = "", status: Int = 200, as name: String = "main") async throws {
        _ = try await fetch(url: "\(base)/set/\(token)-\(name)?items=\(items)&status=\(status)", cookies: nil)
    }

    private func titles(_ articles: [Article]) -> [String] {
        articles.map(\.title)
    }

    func testAddingAFeedFetchesItFirst() async throws {
        try await serve("old:1,new:2")
        let model = model()
        XCTAssertEqual(model.feeds, [])

        let added = await model.addFeed(name: "", url: " \(feedURL()) ", cookies: "")
        XCTAssertTrue(added, model.errorMessage ?? "")
        XCTAssertEqual(model.feeds.map(\.name), ["Feed \(token)-main"], "the feed's own title, when no name is given")
        XCTAssertEqual(model.feeds.map(\.url), [feedURL()])
        XCTAssertEqual(titles(model.articles), ["new", "old"])
        XCTAssertEqual(model.articles[0].link, "https://ex.com/\(token)-main/new")
        XCTAssertEqual(model.articles[0].summary, "About new")
        XCTAssertEqual(store.loadFeeds(), model.feeds)
        XCTAssertNil(model.errorMessage)

        // Twice the same feed is once.
        let again = await model.addFeed(name: "Again", url: feedURL(), cookies: "")
        XCTAssertFalse(again)
        XCTAssertEqual(model.errorMessage, "this feed is already in the list, as Feed \(token)-main")
        XCTAssertEqual(model.feeds.count, 1)
    }

    func testWhatIsNotAFeedIsNotAdded() async throws {
        let model = model()
        for (url, expected) in [
            ("\(base)/page.html", "not a feed: this is a web page: look on it for a link named RSS, Atom or feed"),
            ("\(base)/feed/\(token)-nothing.xml", "the server answered 404"),
            ("ftp://ex.com/feed", "the URL of a feed starts with http:// or https://"),
            ("", "the feed has no URL"),
        ] {
            model.errorMessage = nil
            let added = await model.addFeed(name: "Nope", url: url, cookies: "")
            XCTAssertFalse(added, url)
            // The core's own sentence, not the name of an enum case.
            XCTAssertEqual(model.errorMessage, expected)
        }
        let added = await model.addFeed(name: "Nope", url: feedURL(), cookies: "a=b\r\nX-Evil: 1")
        XCTAssertFalse(added)
        XCTAssertEqual(model.errorMessage, "the cookies are one line of name=value pairs, like: session=abc; user=42")

        XCTAssertEqual(model.feeds, [])
        XCTAssertEqual(store.loadFeeds(), [])
        XCTAssertTrue(secrets.isEmpty)
    }

    func testRefreshMergesEveryFeedNewestFirst() async throws {
        try await serve("a1:1,a3:3", as: "a")
        try await serve("b2:2,b4:4,undated:0", as: "b")
        let model = model()
        _ = await model.addFeed(name: "A", url: feedURL("a"), cookies: "")
        _ = await model.addFeed(name: "B", url: feedURL("b"), cookies: "")
        // 2024 is after the test's clock (2023): the undated article sorts by the day it was seen.
        XCTAssertEqual(titles(model.articles), ["b4", "a3", "b2", "a1", "undated"])
        let (a, b) = (model.feeds[0], model.feeds[1])

        // feed.py's test_dropped_article_persists_in_cache: a1 is gone from the feed, not from the phone.
        try await serve("a3:3,a5:5", as: "a")
        clock.advance(3600)
        await model.refresh()
        XCTAssertNil(model.errorMessage)
        XCTAssertFalse(model.isRefreshing)
        XCTAssertEqual(titles(model.articles), ["a5", "b4", "a3", "b2", "a1", "undated"])
        XCTAssertEqual(titles(model.articles(in: .feed(a.id))), ["a5", "a3", "a1"])
        XCTAssertEqual(titles(model.articles(in: .feed(b.id))), ["b4", "b2", "undated"])
        XCTAssertEqual(model.articles.first { $0.title == "a5" }?.firstSeen, clock.now)
        XCTAssertEqual(model.articles.first { $0.title == "a3" }?.firstSeen, clock.now - 3600)

        model.filter = .feed(b.id)
        XCTAssertEqual(titles(model.visibleArticles), ["b4", "b2", "undated"])
        XCTAssertEqual(model.filterName, "B")
        model.filter = .all
        XCTAssertEqual(model.filterName, "All articles")
    }

    func testReadStateAndTheCacheSurviveARestartWithNoNetwork() async throws {
        try await serve("one:1,two:2,three:3")
        let first = model()
        _ = await first.addFeed(name: "Mine", url: feedURL(), cookies: "")
        let feed = first.feeds[0]
        XCTAssertEqual(first.unreadCount(in: .all), 3)

        let two = first.articles[1]
        first.setRead(true, two)
        first.setRead(true, two) // twice is once
        XCTAssertTrue(first.isRead(two))
        XCTAssertEqual(first.unreadCount(in: .feed(feed.id)), 2)
        first.unreadOnly = true
        XCTAssertEqual(titles(first.visibleArticles), ["three", "one"])

        // The server is down. A new model is the app starting again: it shows what it had, at once.
        try await serve(status: 500)
        let second = model()
        XCTAssertEqual(second.feeds, [feed])
        XCTAssertEqual(second.articles, first.articles)
        XCTAssertEqual(second.readKeys, [two.key])
        XCTAssertFalse(second.unreadOnly, "a filter is not a setting")

        // feed.py's test_failed_feed_doesnt_lose_cached_articles
        await second.refresh(quiet: true)
        XCTAssertNil(second.errorMessage, "the refresh on launch raises no alert")
        XCTAssertEqual(second.failures, [feed.id: "the server answered 500"])
        XCTAssertEqual(titles(second.articles), ["three", "two", "one"])
        await second.refresh()
        XCTAssertEqual(second.errorMessage, "Mine: the server answered 500")
        XCTAssertEqual(second.readKeys, [two.key], "a failure forgets nothing")

        // It is back: the failure is forgotten.
        try await serve("one:1,two:2,three:3,four:4")
        await second.refresh()
        XCTAssertEqual(second.failures, [:])
        XCTAssertEqual(titles(second.articles), ["four", "three", "two", "one"])

        second.toggleRead(two)
        XCTAssertFalse(second.isRead(two))
        second.toggleRead(two)
        second.filter = .feed(feed.id)
        second.markAllRead()
        XCTAssertEqual(second.unreadCount(in: .all), 0)
        XCTAssertEqual(model().readKeys.count, 4)
    }

    func testMarkAllReadOnlyMarksWhatIsShown() async throws {
        try await serve("a1:1", as: "a")
        try await serve("b1:2", as: "b")
        let model = model()
        _ = await model.addFeed(name: "A", url: feedURL("a"), cookies: "")
        _ = await model.addFeed(name: "B", url: feedURL("b"), cookies: "")
        model.filter = .feed(model.feeds[0].id)
        model.markAllRead()
        XCTAssertEqual(model.unreadCount(in: .feed(model.feeds[0].id)), 0)
        XCTAssertEqual(model.unreadCount(in: .feed(model.feeds[1].id)), 1)
    }

    func testCookiesAreSentToTheFeedAndKeptInTheSecretStore() async throws {
        let model = model()
        let refused = await model.addFeed(name: "Private", url: "\(base)/private.xml", cookies: "")
        XCTAssertFalse(refused)
        XCTAssertEqual(model.errorMessage, "the server answered 403")
        XCTAssertTrue(secrets.isEmpty, "nothing is kept of a feed that was not added")

        let added = await model.addFeed(name: "Private", url: "\(base)/private.xml", cookies: " session=abc ")
        XCTAssertTrue(added)
        let feed = model.feeds[0]
        XCTAssertEqual(titles(model.articles), ["members only"])
        XCTAssertEqual(model.cookies(of: feed), "session=abc")
        XCTAssertEqual(store.cookies(for: feed.id), "session=abc")
        XCTAssertFalse(String(decoding: try JSONEncoder().encode(store.loadFeeds()), as: UTF8.self).contains("abc"))

        // The refresh sends them too.
        await model.refresh()
        XCTAssertEqual(model.failures, [:])

        // Wrong cookies are not saved over the right ones.
        let changed = await model.updateFeed(feed, name: "Private", url: feed.url, cookies: "session=wrong")
        XCTAssertFalse(changed)
        XCTAssertEqual(store.cookies(for: feed.id), "session=abc")

        model.removeFeed(feed)
        XCTAssertTrue(secrets.isEmpty, "the cookies go with the feed")
    }

    func testCookiesDoNotFollowARedirectToAnotherHost() async throws {
        let model = model()
        _ = await model.addFeed(name: "Same", url: "\(base)/redirect/same", cookies: "session=secret")
        _ = await model.addFeed(name: "Other", url: "\(base)/redirect/other", cookies: "session=secret")
        XCTAssertEqual(model.feeds.count, 2, model.errorMessage ?? "")
        let titles = Dictionary(uniqueKeysWithValues: model.feeds.map { feed in
            (feed.name, model.articles(in: .feed(feed.id)).map(\.title))
        })
        XCTAssertEqual(titles["Same"], ["cookie: session=secret"])
        XCTAssertEqual(titles["Other"], ["cookie: none"])
    }

    func testEditingAFeed() async throws {
        try await serve("old1:1,old2:2", as: "old")
        try await serve("new1:3", as: "new")
        let model = model()
        _ = await model.addFeed(name: "Blog", url: feedURL("old"), cookies: "")
        let feed = model.feeds[0]
        model.setRead(true, model.articles[0])

        // A new name is only a new name: nothing is fetched (the server would now fail).
        try await serve(status: 500, as: "old")
        let renamed = await model.updateFeed(feed, name: " Journal ", url: feed.url, cookies: "")
        XCTAssertTrue(renamed, model.errorMessage ?? "")
        XCTAssertEqual(model.feeds, [Feed(id: feed.id, name: "Journal", url: feed.url)])
        XCTAssertEqual(titles(model.articles), ["old2", "old1"])
        XCTAssertEqual(model.readKeys.count, 1)

        // A new URL is tried before it replaces the old one.
        let broken = await model.updateFeed(model.feeds[0], name: "Journal", url: "\(base)/page.html", cookies: "")
        XCTAssertFalse(broken)
        XCTAssertEqual(model.feeds[0].url, feed.url)
        XCTAssertEqual(titles(model.articles), ["old2", "old1"])

        // Moved: it starts again from what is there.
        let moved = await model.updateFeed(model.feeds[0], name: "", url: feedURL("new"), cookies: "")
        XCTAssertTrue(moved)
        XCTAssertEqual(model.feeds, [Feed(id: feed.id, name: "Feed \(token)-new", url: feedURL("new"))])
        XCTAssertEqual(titles(model.articles), ["new1"])
        XCTAssertEqual(model.readKeys, [], "what is read of articles that are gone is forgotten")
        XCTAssertEqual(store.loadFeeds(), model.feeds)
        XCTAssertEqual(titles(library.articles(of: feed.id)), ["new1"])
    }

    func testRemovingAFeedRemovesWhatItHad() async throws {
        try await serve("a1:1", as: "a")
        try await serve("b1:2", as: "b")
        let model = model()
        _ = await model.addFeed(name: "A", url: feedURL("a"), cookies: "k=v")
        _ = await model.addFeed(name: "B", url: feedURL("b"), cookies: "")
        let (a, b) = (model.feeds[0], model.feeds[1])
        model.markAllRead()
        model.filter = .feed(a.id)

        model.removeFeed(a)
        XCTAssertEqual(model.feeds, [b])
        XCTAssertEqual(titles(model.articles), ["b1"])
        XCTAssertEqual(model.filter, .all, "the filter does not stay on a feed that is gone")
        XCTAssertEqual(model.readKeys, [model.articles[0].key])
        XCTAssertEqual(library.articles(of: a.id), [])
        XCTAssertTrue(secrets.isEmpty)
        XCTAssertEqual(self.model().feeds, [b])
    }

    func testAnArticleIsShownAsASafePage() async throws {
        let model = model()
        let added = await model.addFeed(name: "Hostile <b>", url: "\(base)/hostile.xml", cookies: "")
        XCTAssertTrue(added, model.errorMessage ?? "")
        let article = try XCTUnwrap(model.articles.first { $0.link == "https://ex.com/blog/post" })
        XCTAssertEqual(article.title, "<script>alert(0)</script>", "a title is text")
        XCTAssertEqual(article.summary, "Hello us x")
        XCTAssertEqual(article.webURL?.absoluteString, "https://ex.com/blog/post")
        XCTAssertEqual(model.article(id: article.id), article)

        // What the feed wrote is kept as it is, and never shown as it is.
        XCTAssertTrue(article.content.contains("<script>alert(3)</script>"))
        var page = model.page(for: article)
        for forbidden in ["<script", "alert(3)", "onclick", "onerror", "javascript:", "<iframe", "evil.example", "<style>body", "<img", "<b>"] {
            XCTAssertFalse(page.contains(forbidden), forbidden)
        }
        XCTAssertTrue(page.contains("<h1>&lt;script&gt;alert(0)&lt;/script&gt;</h1>"))
        XCTAssertTrue(page.contains("<p>Hostile &lt;b&gt; · "), "the byline is text too")
        XCTAssertTrue(page.contains("<p>Hello <a href=\"https://ex.com/about\" rel=\"noopener noreferrer\">us</a> <a>x</a></p>"))
        XCTAssertTrue(page.contains("<span class=\"image\">A pic</span>"))
        XCTAssertTrue(page.contains("default-src 'none'; style-src 'unsafe-inline'; img-src 'none';"))

        // Images are a choice, and then only over https.
        XCTAssertFalse(model.loadImages)
        model.setLoadImages(true)
        XCTAssertTrue(self.model().loadImages)
        page = model.page(for: article)
        XCTAssertTrue(page.contains("<img src=\"https://ex.com/blog/pic.png\" alt=\"A pic\" referrerpolicy=\"no-referrer\">"))
        XCTAssertTrue(page.contains("img-src https:;"))
        XCTAssertFalse(page.contains("onerror"))

        // A link that is not a web link is never one to open.
        let other = try XCTUnwrap(model.articles.first { $0.title == "Not a web link" })
        XCTAssertEqual(other.link, "javascript:alert(5)")
        XCTAssertNil(other.webURL)
        XCTAssertFalse(model.page(for: other).contains("javascript:"))
    }

    func testAtomFeedsAndBylines() async throws {
        let model = model()
        let added = await model.addFeed(name: "", url: "\(base)/atom.xml", cookies: "")
        XCTAssertTrue(added, model.errorMessage ?? "")
        XCTAssertEqual(model.feeds[0].name, "Atom fixture")
        let article = model.articles[0]
        XCTAssertEqual([article.title, article.link, article.author, article.content], ["Atom entry", "https://ex.com/atom/1", "Bob", "<p>atom body</p>"])
        XCTAssertEqual(article.sortDate, Date(timeIntervalSince1970: 1_706_954_400))
        let byline = model.byline(for: article)
        XCTAssertTrue(byline.hasPrefix("Atom fixture · Bob · "), byline)
        XCTAssertTrue(byline.contains("2024"), byline)
    }
}
