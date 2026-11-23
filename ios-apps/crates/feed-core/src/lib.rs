//! RSS and Atom feeds (from <https://github.com/c4ffein/feed>), exposed to Swift through UniFFI:
//! fetching one, reading its articles, and making the HTML of an article safe to show.
//!
//! [`fetch_feed`] blocks until the server answers: call it off the main thread.
//! Nothing here keeps any state: the list of feeds, what is read and the cache are the app's.

use std::borrow::Cow;
use std::io::Read;
use std::time::Duration;

use roxmltree::{Document, Node, ParsingOptions};
use url::Url;

mod dates;
pub mod html;

pub use dates::parse_date;

uniffi::setup_scaffolding!();

/// For one request, body included.
const TIMEOUT: Duration = Duration::from_secs(30);
/// A feed is some hundred kilobytes. Past this it is not read: it would all be held in memory.
const MAX_BODY: u64 = 10 * 1024 * 1024;
/// Elements, attributes and texts in one document: bounds what the parsed tree takes.
const MAX_NODES: u32 = 500_000;
const MAX_REDIRECTS: usize = 5;
const USER_AGENT: &str = concat!("feed-core/", env!("CARGO_PKG_VERSION"));
const EXCERPT_LENGTH: usize = 300;

const ATOM: &str = "http://www.w3.org/2005/Atom";
const RSS1: &str = "http://purl.org/rss/1.0/";
const CONTENT: &str = "http://purl.org/rss/1.0/modules/content/";
const DC: &str = "http://purl.org/dc/elements/1.1/";

/// One article of a feed: feed.py's `Entry`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Entry {
    /// What read/unread is kept by: the URL-safe base64 of the link, as feed.py's `get_article_id`
    /// (of the guid or Atom id when there is no link).
    pub id: String,
    /// One line of text: see [`sanitize_title`].
    pub title: String,
    /// Absolute when the feed's own URL is known. Not checked: only open it when it is http(s).
    pub link: String,
    /// As written in the feed.
    pub date: String,
    /// `date` as Unix seconds, when it is RFC 822 or ISO 8601.
    pub timestamp: Option<i64>,
    pub author: String,
    /// The start of the description, as plain text: for a list.
    pub summary: String,
    /// The full content when the feed has it, the description otherwise.
    /// HTML, **as the feed wrote it**: only show it through [`article_page`].
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ParsedFeed {
    /// The feed's own name for itself, sanitized like an article title. May be empty.
    pub title: String,
    /// In the order of the feed, at most one per id.
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum FeedError {
    #[error("{message}")]
    InvalidUrl { message: String },
    #[error("the cookies are one line of name=value pairs, like: session=abc; user=42")]
    InvalidCookies,
    #[error("{message}")]
    Network { message: String },
    #[error("the server answered {status}")]
    Http { status: u16 },
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("too large to be a feed (more than 10 MB)")]
    TooLarge,
    #[error("not a feed: {message}")]
    NotAFeed { message: String },
}

fn invalid_url(message: impl Into<String>) -> FeedError {
    FeedError::InvalidUrl {
        message: message.into(),
    }
}

/// feed.py's title allowlist: printable ASCII and the Latin-1 accented letters
/// (À-Ö, Ø-ö, ø-ÿ: not the multiplication and division signs).
fn title_allows(c: char) -> bool {
    matches!(c as u32, 0x20..=0x7e | 0xc0..=0xd6 | 0xd8..=0xf6 | 0xf8..=0xff)
}

/// feed.py's `sanitize_title`: a title comes straight from the feed and is shown on one line.
/// Newlines and tabs become a space, anything off the allowlist (CJK, emoji, symbols, controls)
/// becomes U+FFFD, whitespace runs collapse and the ends are trimmed.
#[uniffi::export]
pub fn sanitize_title(title: String) -> String {
    let replaced: String = title
        .chars()
        .map(|c| match c {
            '\n' | '\r' | '\t' => ' ',
            c if title_allows(c) => c,
            _ => '\u{fffd}',
        })
        .collect();
    replaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Python's `base64.urlsafe_b64encode`, padding included.
fn base64_urlsafe(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, b| n << 8 | u32::from(*b)) << (8 * (3 - chunk.len()));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Windows-1252, where it is not Latin-1 (0x80 to 0x9f).
const CP1252: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘', '’',
    '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
];

/// The document as text. UTF-8, or UTF-16 with its byte order mark; what is neither is read
/// as Windows-1252 (and so Latin-1), the one other encoding feeds are still written in.
fn decode(bytes: &[u8]) -> Cow<'_, str> {
    let utf16 = |bytes: &[u8], read: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = bytes.chunks_exact(2).map(|pair| read([pair[0], pair[1]])).collect();
        Cow::Owned(String::from_utf16_lossy(&units))
    };
    match bytes {
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8_lossy(rest),
        [0xff, 0xfe, rest @ ..] => utf16(rest, u16::from_le_bytes),
        [0xfe, 0xff, rest @ ..] => utf16(rest, u16::from_be_bytes),
        _ => match std::str::from_utf8(bytes) {
            Ok(text) => Cow::Borrowed(text),
            Err(_) => Cow::Owned(
                bytes
                    .iter()
                    .map(|b| match b {
                        0x80..=0x9f => CP1252[usize::from(b - 0x80)],
                        b => char::from(*b),
                    })
                    .collect(),
            ),
        },
    }
}

/// The first child element of that name, in one of these namespaces (`""`: none).
fn child<'a, 'input>(node: Node<'a, 'input>, namespaces: &[&str], name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|c| {
        c.is_element() && c.tag_name().name() == name && namespaces.contains(&c.tag_name().namespace().unwrap_or(""))
    })
}

/// ElementTree's `findtext(name, "")`: the text at the start of that child.
fn text<'a>(node: Node<'a, '_>, namespaces: &[&str], name: &str) -> &'a str {
    child(node, namespaces, name).and_then(|c| c.text()).unwrap_or("")
}

/// Every descendant element of that name.
fn elements<'a, 'input>(
    doc: &'a Document<'input>,
    namespaces: &'a [&str],
    name: &'a str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    doc.descendants().filter(move |n| {
        n.is_element() && n.tag_name().name() == name && namespaces.contains(&n.tag_name().namespace().unwrap_or(""))
    })
}

/// What a parser hands over for one article, before it is made an [`Entry`].
struct Raw<'a> {
    title: &'a str,
    link: &'a str,
    /// The feed's own identifier for the article: only used when there is no link.
    guid: &'a str,
    date: &'a str,
    author: &'a str,
    description: Cow<'a, str>,
    content: Cow<'a, str>,
}

impl Raw<'_> {
    /// `None` for an article with neither a link nor an identifier: nothing to tell it by.
    fn entry(self, base: Option<&Url>) -> Option<Entry> {
        // An absolute link is kept as written (its id is then feed.py's), a relative one is resolved.
        let mut link = self.link.trim().to_string();
        if let Some(base) = base.filter(|_| !link.is_empty()) {
            if Url::parse(&link) == Err(url::ParseError::RelativeUrlWithoutBase) {
                link = base.join(&link).map_or(link, String::from);
            }
        }
        let key = if link.is_empty() { self.guid.trim() } else { &link };
        if key.is_empty() {
            return None;
        }
        Some(Entry {
            id: base64_urlsafe(key.as_bytes()),
            title: sanitize_title(self.title.to_string()),
            date: self.date.trim().to_string(),
            timestamp: parse_date(self.date),
            author: self.author.trim().to_string(),
            summary: html::excerpt(&self.description, EXCERPT_LENGTH),
            content: self.content.into_owned(),
            link,
        })
    }
}

/// feed.py's `get_rss_entries`, plus RSS 1.0 (whose items are in a namespace).
fn rss_entries<'a>(doc: &'a Document<'_>) -> impl Iterator<Item = Raw<'a>> {
    const RSS: &[&str] = &["", RSS1];
    elements(doc, RSS, "item").map(|item| {
        let description = text(item, RSS, "description");
        let content = match text(item, &[CONTENT], "encoded") {
            "" => description,
            content => content,
        };
        let author = match text(item, &[DC], "creator") {
            "" => text(item, RSS, "author"),
            creator => creator,
        };
        let date = match text(item, RSS, "pubDate") {
            "" => text(item, &[DC], "date"),
            date => date,
        };
        Raw {
            title: text(item, RSS, "title"),
            link: text(item, RSS, "link"),
            guid: text(item, RSS, "guid"),
            date,
            author,
            description: description.into(),
            content: content.into(),
        }
    })
}

/// The HTML of an Atom text construct (`content`, `summary`), by its `type`.
/// feed.py takes the text whatever the type, which loses `xhtml` content altogether.
fn atom_html<'a>(node: Node<'a, '_>, source: &'a str) -> Cow<'a, str> {
    match node.attribute("type") {
        // The markup is the element's children: taken as written, the sanitizer reads it like any HTML.
        Some("xhtml") => match (node.first_child(), node.last_child()) {
            (Some(first), Some(last)) => source[first.range().start..last.range().end].into(),
            _ => "".into(),
        },
        Some("text") => html::escape(node.text().unwrap_or("")).into(),
        // "html", and no type at all: by the letter that one is text, but feeds put HTML there.
        _ => node.text().unwrap_or("").into(),
    }
}

/// feed.py's `get_atom_entries`.
fn atom_entries<'a>(doc: &'a Document<'_>, source: &'a str) -> impl Iterator<Item = Raw<'a>> {
    const NS: &[&str] = &[ATOM, ""];
    elements(doc, NS, "entry").map(move |item| {
        let links = || {
            item.children().filter(|c| {
                c.is_element() && c.tag_name().name() == "link" && NS.contains(&c.tag_name().namespace().unwrap_or(""))
            })
        };
        // The page of the article (rel="alternate", which is also what no rel means), or else the first link.
        let link = links()
            .find(|l| l.attribute("rel").unwrap_or("alternate") == "alternate")
            .or_else(|| links().next())
            .and_then(|l| l.attribute("href"))
            .unwrap_or("");
        let body = [child(item, NS, "content"), child(item, NS, "summary")]
            .into_iter()
            .flatten()
            .map(|node| atom_html(node, source))
            .find(|html| !html.is_empty())
            .unwrap_or_default();
        let date = match text(item, NS, "published") {
            "" => text(item, NS, "updated"),
            published => published,
        };
        Raw {
            title: text(item, NS, "title"),
            link,
            guid: text(item, NS, "id"),
            date,
            author: child(item, NS, "author").map_or("", |author| text(author, NS, "name")),
            description: body.clone(),
            content: body,
        }
    })
}

fn not_a_feed(message: impl Into<String>) -> FeedError {
    FeedError::NotAFeed {
        message: message.into(),
    }
}

fn parse(bytes: &[u8], base: Option<&Url>) -> Result<ParsedFeed, FeedError> {
    let source = decode(bytes);
    // A DTD is allowed (old RSS has one), and harmless: the parser reads no external entity
    // and refuses the entities that expand into one another without end.
    let options = ParsingOptions {
        allow_dtd: true,
        nodes_limit: MAX_NODES,
        ..ParsingOptions::default()
    };
    let doc = Document::parse_with_options(&source, options).map_err(|e| {
        let looks_like_html = source.trim_start().get(..64).unwrap_or(&source).to_ascii_lowercase();
        if looks_like_html.starts_with("<!doctype html") || looks_like_html.starts_with("<html") {
            not_a_feed("this is a web page: look on it for a link named RSS, Atom or feed")
        } else {
            not_a_feed(format!("invalid XML ({e})"))
        }
    })?;
    let root = doc.root_element().tag_name();
    let (title, raw): (&str, Box<dyn Iterator<Item = Raw>>) = match (root.name(), root.namespace()) {
        ("feed", Some(ATOM) | None) => (
            text(doc.root_element(), &[ATOM, ""], "title"),
            Box::new(atom_entries(&doc, &source)),
        ),
        ("rss" | "RDF", _) => (
            elements(&doc, &["", RSS1], "channel")
                .next()
                .map_or("", |channel| text(channel, &["", RSS1], "title")),
            Box::new(rss_entries(&doc)),
        ),
        ("html", _) => {
            return Err(not_a_feed(
                "this is a web page: look on it for a link named RSS, Atom or feed",
            ))
        }
        (other, _) => return Err(not_a_feed(format!("the document is a <{other}>, neither RSS nor Atom"))),
    };
    let mut entries: Vec<Entry> = Vec::new();
    for entry in raw.filter_map(|raw| raw.entry(base)) {
        if !entries.iter().any(|known| known.id == entry.id) {
            entries.push(entry);
        }
    }
    Ok(ParsedFeed {
        title: sanitize_title(title.to_string()),
        entries,
    })
}

/// Checks a feed URL and returns it as it will be used: http or https (https when none is written),
/// a host, and no `user:password@` (the URL is not kept as a secret: cookies are).
#[uniffi::export]
pub fn check_feed_url(url: String) -> Result<String, FeedError> {
    Ok(parse_url(&url)?.into())
}

fn parse_url(url: &str) -> Result<Url, FeedError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(invalid_url("the feed has no URL"));
    }
    if url.contains(|c: char| c.is_whitespace() || c.is_control()) {
        return Err(invalid_url("there is a space in the URL"));
    }
    let with_scheme = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };
    let parsed = Url::parse(&with_scheme).map_err(|e| invalid_url(format!("not a URL: {e}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(invalid_url("the URL of a feed starts with http:// or https://"));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(invalid_url("the URL has no host"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid_url("a user and password in the URL are not supported"));
    }
    Ok(parsed)
}

/// May the cookies given for `feed` go with a request to `to`? Only to the same server:
/// same host, same scheme and port. The one exception is the usual upgrade, from http to https
/// on the default ports. Never to another host, and never from https back to plain http.
fn cookies_may_go(feed: &Url, to: &Url) -> bool {
    let same_server = feed.scheme() == to.scheme() && feed.port_or_known_default() == to.port_or_known_default();
    let upgrade = feed.scheme() == "http" && to.scheme() == "https" && feed.port().is_none() && to.port().is_none();
    feed.host_str() == to.host_str() && (same_server || upgrade)
}

/// One line that cannot end the header it goes in, or start another.
fn checked_cookies(cookies: Option<String>) -> Result<Option<String>, FeedError> {
    let Some(cookies) = cookies.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    if cookies.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
        Ok(Some(cookies))
    } else {
        Err(FeedError::InvalidCookies)
    }
}

/// Reads RSS or Atom. `base_url` is where the document comes from: relative links are resolved against it.
#[uniffi::export]
pub fn parse_feed(data: Vec<u8>, base_url: Option<String>) -> Result<ParsedFeed, FeedError> {
    let base = base_url.as_deref().map(parse_url).transpose()?;
    parse(&data, base.as_ref())
}

/// Fetches and reads a feed. `cookies` is the value of a `Cookie` header (`session=abc; user=42`),
/// for a feed behind a login.
///
/// Redirects are followed here, one by one (at most 5), and not by the HTTP client: it would send
/// the same headers to wherever the server points. The cookies only go to the feed's own server
/// (see `cookies_may_go`): a redirect to another host is followed without them.
/// https is verified against the Mozilla roots, as a browser would.
#[uniffi::export]
pub fn fetch_feed(url: String, cookies: Option<String>) -> Result<ParsedFeed, FeedError> {
    let feed = parse_url(&url)?;
    let cookies = checked_cookies(cookies)?;
    let agent = pinned_http::web_agent(0).map_err(|e| FeedError::Network { message: e.to_string() })?;
    let mut current = feed.clone();
    for _ in 0..=MAX_REDIRECTS {
        let mut request = agent
            .request_url("GET", &current)
            .timeout(TIMEOUT)
            .set("User-Agent", USER_AGENT)
            .set(
                "Accept",
                "application/atom+xml, application/rss+xml, application/xml;q=0.9, text/xml;q=0.9, */*;q=0.5",
            );
        if let Some(cookies) = cookies.as_deref().filter(|_| cookies_may_go(&feed, &current)) {
            request = request.set("Cookie", cookies);
        }
        let response = match request.call() {
            Ok(response) => response,
            Err(ureq::Error::Status(status, _)) => return Err(FeedError::Http { status }),
            Err(ureq::Error::Transport(transport)) => {
                return Err(FeedError::Network {
                    message: transport.to_string(),
                })
            }
        };
        let status = response.status();
        if (200..300).contains(&status) {
            let mut body = Vec::new();
            response
                .into_reader()
                .take(MAX_BODY + 1)
                .read_to_end(&mut body)
                .map_err(|e| FeedError::Network { message: e.to_string() })?;
            if body.len() as u64 > MAX_BODY {
                return Err(FeedError::TooLarge);
            }
            return parse(&body, Some(&current));
        }
        // The agent follows nothing, so a 3xx comes back as a response.
        let location = match (status, response.header("Location")) {
            (301 | 302 | 303 | 307 | 308, Some(location)) => location,
            _ => return Err(FeedError::Http { status }),
        };
        let mut next = current
            .join(location)
            .map_err(|e| invalid_url(format!("redirected to what is not a URL: {e}")))?;
        if !matches!(next.scheme(), "http" | "https") || next.host_str().is_none_or(str::is_empty) {
            return Err(invalid_url(format!("redirected to {}: not http(s)", next.scheme())));
        }
        // A server does not get to make this send a password either.
        let _ = next.set_username("");
        let _ = next.set_password(None);
        current = next;
    }
    Err(FeedError::TooManyRedirects)
}

/// The document a web view shows for an article. `title` and `byline` are text. `content` is the
/// article's HTML as the feed wrote it: it is rewritten from an allowlist (see `html.rs`), and its
/// relative links are resolved against `link`. The page's Content-Security-Policy lets nothing load,
/// but, when `images` is set, images over https.
#[uniffi::export]
pub fn article_page(title: String, byline: String, content: String, link: String, images: bool) -> String {
    let base = Url::parse(link.trim()).ok();
    let body = html::sanitize(&content, base.as_ref(), images);
    html::page(&title, &byline, &body, images)
}

/// The message of an error, for people. (The Swift `localizedDescription` of a
/// generated error is its case name, not this.)
#[uniffi::export]
pub fn error_message(error: FeedError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// feed.py's tests/test_sanitize_title.py, case by case.
    #[test]
    fn titles_are_sanitized() {
        for (title, expected) in [
            ("hello\nworld", "hello world"),
            ("a\rb\tc", "a b c"),
            ("  a\n\n\tb  ", "a b"),
            (
                "Hello, World! (2024) #1 @ 50% -/+ [x]",
                "Hello, World! (2024) #1 @ 50% -/+ [x]",
            ),
            ("café résumé naïve àÿ", "café résumé naïve àÿ"),
            ("日本", "��"),
            ("party🎉time", "party�time"),
            ("10€ © ×", "10� � �"),
            ("a\x07b", "a�b"),
            ("x\x7fy\u{9f}z", "x�y�z"),
            ("", ""),
        ] {
            assert_eq!(sanitize_title(title.into()), expected, "{title:?}");
        }
    }

    #[test]
    fn ids_are_feed_py_ids() {
        // base64.urlsafe_b64encode(b"https://ex.com/a?b=c>>d??").decode()
        assert_eq!(
            base64_urlsafe(b"https://ex.com/a?b=c>>d??"),
            "aHR0cHM6Ly9leC5jb20vYT9iPWM-PmQ_Pw=="
        );
        assert_eq!(base64_urlsafe(b""), "");
        assert_eq!(base64_urlsafe(b"f"), "Zg==");
        assert_eq!(base64_urlsafe(b"fo"), "Zm8=");
        assert_eq!(base64_urlsafe(b"foo"), "Zm9v");
        assert_eq!(base64_urlsafe(&[0xfb, 0xff, 0xfe]), "-__-");
    }

    #[test]
    fn urls_are_checked() {
        assert_eq!(
            check_feed_url(" https://ex.com/feed.xml\n".into()).unwrap(),
            "https://ex.com/feed.xml"
        );
        assert_eq!(check_feed_url("ex.com/feed".into()).unwrap(), "https://ex.com/feed");
        assert_eq!(check_feed_url("ex.com:8080".into()).unwrap(), "https://ex.com:8080/");
        assert_eq!(
            check_feed_url("http://10.0.0.2:8080/rss?a=b".into()).unwrap(),
            "http://10.0.0.2:8080/rss?a=b"
        );
        assert_eq!(check_feed_url("HTTPS://EX.com".into()).unwrap(), "https://ex.com/");
        for url in [
            "",
            "   ",
            "https://",
            "ftp://ex.com/feed",
            "file:///etc/passwd",
            "javascript://ex.com/%0aalert(1)",
            "https://user:pw@ex.com/feed",
            "https://user@ex.com/feed",
            "https://ex.com/a b",
            "https://ex.com/a\r\nX-Evil: 1",
        ] {
            assert!(
                matches!(check_feed_url(url.into()), Err(FeedError::InvalidUrl { .. })),
                "{url:?}"
            );
        }
    }

    #[test]
    fn cookies_only_go_to_the_server_of_the_feed() {
        let go = |feed: &str, to: &str| cookies_may_go(&Url::parse(feed).unwrap(), &Url::parse(to).unwrap());
        assert!(go("https://ex.com/feed", "https://ex.com/feed"));
        assert!(go("https://ex.com/feed", "https://EX.com:443/elsewhere?x"));
        assert!(go("http://ex.com/feed", "https://ex.com/feed"), "the usual upgrade");
        assert!(go("http://ex.com:8080/feed", "http://ex.com:8080/other"));

        assert!(!go("https://ex.com/feed", "https://evil.example/feed"));
        assert!(!go("https://ex.com/feed", "https://sub.ex.com/feed"));
        assert!(!go("https://sub.ex.com/feed", "https://ex.com/feed"));
        assert!(!go("https://ex.com/feed", "https://ex.com.evil.example/feed"));
        assert!(
            !go("https://ex.com/feed", "http://ex.com/feed"),
            "never back to plain http"
        );
        assert!(!go("https://ex.com/feed", "https://ex.com:8443/feed"));
        assert!(!go("http://ex.com:8080/feed", "http://ex.com:8081/feed"));
        assert!(!go("http://ex.com:8080/feed", "https://ex.com/feed"));
        assert!(!go("http://127.0.0.1/feed", "http://localhost/feed"));
    }

    #[test]
    fn cookies_are_one_header_value() {
        assert_eq!(checked_cookies(None), Ok(None));
        assert_eq!(checked_cookies(Some("  ".into())), Ok(None));
        assert_eq!(
            checked_cookies(Some(" session=abc; user=42\n".into())),
            Ok(Some("session=abc; user=42".into()))
        );
        assert_eq!(
            checked_cookies(Some("a=b\r\nX-Evil: 1".into())),
            Err(FeedError::InvalidCookies)
        );
        assert_eq!(checked_cookies(Some("a=b\0".into())), Err(FeedError::InvalidCookies));
        assert_eq!(checked_cookies(Some("a=é".into())), Err(FeedError::InvalidCookies));
    }

    #[test]
    fn other_encodings_are_read() {
        assert_eq!(decode(b"caf\xc3\xa9"), "café");
        assert_eq!(decode(b"\xef\xbb\xbfcaf\xc3\xa9"), "café");
        assert_eq!(decode(b"caf\xe9 \x93x\x94 \x80"), "café “x” €");
        assert_eq!(decode(b"\xff\xfec\x00a\x00f\x00\xe9\x00"), "café");
        assert_eq!(decode(b"\xfe\xff\x00c\x00a\x00f\x00\xe9"), "café");
        assert_eq!(decode(b""), "");
    }
}
