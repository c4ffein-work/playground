# Feed

An RSS/Atom reader for iOS: the phone side of [feed](https://github.com/c4ffein/feed), same shape as
[Companion](../companion): `crates/feed-core` (Rust) under `Package/` (Swift, no UI) under `App/` (SwiftUI views).

## What the app does

- **Articles**: every article of every feed, newest first, unread ones marked. Filter on one feed, or on unread only.
  Pull down to refresh; the app also refreshes when it opens, quietly.
- **Offline first**: articles and read state are on disk. The app opens on what it had, before and without any network.
  A feed that fails keeps its articles and says why in the feed list. An article the feed no longer lists stays
  (up to 500 per feed).
- **Reader**: the article's own HTML, made safe (below). Opening an article marks it read; swipe, or the toolbar, to
  mark it unread again. Share it, or open its page in Safari.
- **Feeds**: add (address, optional name, optional cookies), edit, remove. A feed is fetched before it is saved:
  what is in the list is known to work.

Not there: importing a feed.py `config.json` or OPML, finding the feed of a web page from its address, background
refresh and notifications, HTTP caching (`ETag`), feeds behind HTTP authentication, search.

## Decisions worth knowing

**An article is untrusted HTML.** It is never shown as the feed wrote it:

1. The core rewrites it from an allowlist (`crates/feed-core/src/html.rs`): a fixed list of formatting tags with no
   attributes, `a href` and `img src` only as absolute http(s) URLs parsed and written again, text escaped. `script`,
   `style`, `iframe`, `svg`, forms and the like go with their content. The output cannot hold a tag the sanitizer did
   not write itself, so it does not depend on reading broken markup the way a browser does.
2. The page around it carries `Content-Security-Policy: default-src 'none'`: no script, frame, font or stylesheet
   can load whatever got through.
3. The web view has JavaScript off, no cookies or storage (`nonPersistent`), no origin (`about:blank`), and navigates
   nowhere: a tapped link goes to Safari if it is http(s), and is dropped otherwise.

**Remote content is off by default.** An image tells its server who opened what, and when. Out of the box an article
shows `[the image's description]` and loads nothing at all. "Load images" (in Feeds) allows images, over https only,
with no referrer; 1×1 tracking pixels are dropped either way. Nothing else is ever loaded.

**Cookies** (for a feed behind a login) are kept in the keychain, this device only; the list of feeds is in
`UserDefaults` and holds none. They are sent to the feed's own server and nowhere else: redirects are followed by the
core itself, one by one (5 at most), and the `Cookie` header only goes along when host, scheme and port are the
feed's (or for the usual http → https upgrade). A redirect to another host is followed without them, and never from
https down to http. The form warns when cookies are given for a plain `http://` feed. A secret in the URL itself
(`?token=…`) is not protected: it is in `UserDefaults` with the URL.

**https** is verified by the core against the bundled Mozilla roots (`pinned_http::web_agent`): there is nothing to pin
for arbitrary sites. Plain http feeds are allowed, as in feed.py.

**Same as feed.py**: titles (its allowlist: anything but printable ASCII and Latin-1 letters becomes `�`, on one
line), dates (RFC 822 then ISO 8601), ids (URL-safe base64 of the link, what read/unread is keyed by), `content:encoded`
over `description`, `dc:creator` over `author`, Atom's `published` over `updated` and `content` over `summary`.
Its parser, title and date test cases are ported.

**Different from feed.py**, on purpose: an article with no date sorts by the day it was first seen instead of last
(and does not break the sort); a cached article takes the feed's corrections instead of staying as first seen;
relative links are resolved; an entry with no link is kept if it has a guid/id, dropped otherwise; Atom `type="xhtml"`
and `type="text"` content is read as such; RSS 1.0 and Latin-1 feeds are read; a document that is neither RSS nor
Atom is an error instead of an empty feed; a response over 10 MB is refused.

## State

| Part | State |
| :-- | :-- |
| `crates/feed-core` | Tested on Linux: `cargo test -p feed-core`. Fetching against a local HTTP server started by each test, never the internet |
| `Package/` (FeedKit: feeds, cookies, cache, read state, `AppModel`) | Tested on Linux through the generated bindings, against `Package/Tests/fixtures/server.py`: `apps/feed/swift-test.sh`. The keychain code is Apple-only and has not been compiled |
| `App/`, `project.yml` | **Written without a Mac: syntax-checked only, never compiled or run.** Expect to fix things on the first Xcode build |

What the tests cover: parsing (feed.py's fixtures, plus namespaces, Atom content types, RSS 1.0, encodings, entity
bombs), the sanitizer (case by case, and 20 000 generated inputs checked to contain only what it writes), cookies and
redirects (same host, other host, other port, loops, redirects to `file:`), and in Swift: adding/editing/removing
feeds, the merge into the cache, read state across a restart with the server down, cookies never reaching
`UserDefaults`, and the page an article with hostile HTML ends up as.

What they do not: anything on a device. The web view's configuration (JavaScript off, navigation refused), the
keychain, the layout, and https against real sites (the tests are http on 127.0.0.1; the TLS setup is
`pinned-http`'s, tested there) have only been read, not run.

## Build

From `ios-apps/`:

```sh
cargo test -p feed-core
scripts/bindings.sh feed-core feed && apps/feed/swift-test.sh
make xcode APP=feed        # macOS: xcframework + Feed.xcodeproj
```

Before running on a device, replace `dev.cafeine` in `project.yml` with an identifier of yours and set your team.
