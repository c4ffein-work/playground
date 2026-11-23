//! Parsing and fetching, on the fixtures of feed.py's own tests (tests/helper.py, test_parsers.py,
//! test_fetch.py). Nothing here leaves the machine: each test serves its feeds from its own
//! local HTTP server.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use feed_core::{article_page, error_message, fetch_feed, parse_feed, Entry, FeedError, ParsedFeed};

// ---- feed.py's tests/helper.py -------------------------------------------------------------

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// feed.py's `make_rss`. Recognized keys: title, link, pubDate, description, author, dc_creator, content_encoded.
fn make_rss(items: &[&[(&str, &str)]]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <rss version=\"2.0\" xmlns:content=\"http://purl.org/rss/1.0/modules/content/\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
         <channel><title>Test Feed</title><link>https://example.com/</link><description>test</description>",
    );
    for item in items {
        xml.push_str("<item>");
        for (key, value) in item.iter() {
            xml.push_str(&match *key {
                "description" => format!("<description><![CDATA[{value}]]></description>"),
                "dc_creator" => format!("<dc:creator>{}</dc:creator>", escape(value)),
                "content_encoded" => format!("<content:encoded><![CDATA[{value}]]></content:encoded>"),
                key => format!("<{key}>{}</{key}>", escape(value)),
            });
        }
        xml.push_str("</item>");
    }
    xml + "</channel></rss>"
}

/// feed.py's `make_atom`. Recognized keys: title, link, updated, published, summary, content, author_name.
fn make_atom(entries: &[&[(&str, &str)]]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <feed xmlns=\"http://www.w3.org/2005/Atom\"><title>Test Atom Feed</title>\
         <id>urn:test</id><updated>2024-01-01T00:00:00Z</updated>",
    );
    for entry in entries {
        xml.push_str("<entry>");
        for (key, value) in entry.iter() {
            xml.push_str(&match *key {
                "link" => format!("<link rel=\"alternate\" href=\"{}\"/>", escape(value)),
                "content" => format!("<content type=\"html\"><![CDATA[{value}]]></content>"),
                "author_name" => format!("<author><name>{}</name></author>", escape(value)),
                key => format!("<{key}>{}</{key}>", escape(value)),
            });
        }
        xml.push_str("</entry>");
    }
    xml + "</feed>"
}

fn parse(xml: &str) -> ParsedFeed {
    parse_feed(xml.as_bytes().to_vec(), None).unwrap()
}

fn only(xml: &str) -> Entry {
    let mut entries = parse(xml).entries;
    assert_eq!(entries.len(), 1);
    entries.remove(0)
}

fn titles(feed: &ParsedFeed) -> Vec<&str> {
    feed.entries.iter().map(|e| e.title.as_str()).collect()
}

// ---- feed.py's tests/test_parsers.py -------------------------------------------------------

#[test]
fn rss_extracts_basic_fields() {
    let feed = parse(&make_rss(&[&[
        ("title", "Hello"),
        ("link", "https://ex.com/a"),
        ("pubDate", "Mon, 01 Jan 2024 12:00:00 +0000"),
        ("description", "<p>desc</p>"),
        ("author", "alice@ex.com"),
    ]]));
    assert_eq!(feed.title, "Test Feed");
    assert_eq!(
        feed.entries,
        [Entry {
            id: "aHR0cHM6Ly9leC5jb20vYQ==".into(), // base64.urlsafe_b64encode(b"https://ex.com/a")
            title: "Hello".into(),
            link: "https://ex.com/a".into(),
            date: "Mon, 01 Jan 2024 12:00:00 +0000".into(),
            timestamp: Some(1_704_110_400),
            author: "alice@ex.com".into(),
            summary: "desc".into(),
            content: "<p>desc</p>".into(),
        }]
    );
}

#[test]
fn rss_dc_creator_preferred_over_author() {
    let entry = only(&make_rss(&[&[
        ("title", "t"),
        ("link", "https://ex.com/a"),
        ("description", "d"),
        ("author", "fallback"),
        ("dc_creator", "Real Name"),
    ]]));
    assert_eq!(entry.author, "Real Name");
}

#[test]
fn rss_content_encoded_used_when_present() {
    let entry = only(&make_rss(&[&[
        ("title", "t"),
        ("link", "https://ex.com/a"),
        ("description", "short"),
        ("content_encoded", "<p>full content</p>"),
    ]]));
    assert_eq!(entry.content, "<p>full content</p>");
    assert_eq!(entry.summary, "short");
}

#[test]
fn rss_falls_back_to_description_when_no_content() {
    let entry = only(&make_rss(&[&[
        ("title", "t"),
        ("link", "https://ex.com/a"),
        ("description", "just desc"),
    ]]));
    assert_eq!(
        (entry.content.as_str(), entry.summary.as_str()),
        ("just desc", "just desc")
    );
    assert_eq!((entry.date.as_str(), entry.timestamp), ("", None));
}

#[test]
fn atom_extracts_basic_fields() {
    let feed = parse(&make_atom(&[&[
        ("title", "Atom post"),
        ("link", "https://ex.com/atom/1"),
        ("updated", "2024-02-03T10:00:00Z"),
        ("content", "<p>body</p>"),
        ("author_name", "Bob"),
    ]]));
    assert_eq!(feed.title, "Test Atom Feed");
    assert_eq!(
        feed.entries,
        [Entry {
            id: "aHR0cHM6Ly9leC5jb20vYXRvbS8x".into(),
            title: "Atom post".into(),
            link: "https://ex.com/atom/1".into(),
            date: "2024-02-03T10:00:00Z".into(),
            timestamp: Some(1_706_954_400),
            author: "Bob".into(),
            summary: "body".into(),
            content: "<p>body</p>".into(),
        }]
    );
}

#[test]
fn atom_published_preferred_over_updated() {
    let entry = only(&make_atom(&[&[
        ("title", "t"),
        ("link", "https://ex.com/a"),
        ("published", "2024-03-01T00:00:00Z"),
        ("updated", "2024-04-01T00:00:00Z"),
        ("summary", "s"),
    ]]));
    assert_eq!(entry.date, "2024-03-01T00:00:00Z");
}

#[test]
fn atom_summary_used_when_no_content() {
    let entry = only(&make_atom(&[&[
        ("title", "t"),
        ("link", "https://ex.com/a"),
        ("summary", "sum only"),
    ]]));
    assert_eq!(entry.content, "sum only");
}

/// feed.py's test_applied_in_rss_parsing (tests/test_sanitize_title.py).
#[test]
fn titles_are_sanitized_when_parsing() {
    let xml = "<rss><channel><title>The\n feed 🎉</title>\
               <item><title>multi\nline\ttitle</title><link>http://e/x</link></item></channel></rss>";
    let feed = parse(xml);
    assert_eq!(feed.title, "The feed �");
    assert_eq!(titles(&feed), ["multi line title"]);
}

// ---- what feed.py's tests do not cover -----------------------------------------------------

#[test]
fn atom_without_a_namespace_and_its_links() {
    let xml = "<feed><title>Bare</title>\
        <entry><title>one</title><id>urn:1</id>\
          <link rel=\"self\" href=\"https://ex.com/api/1\"/><link rel=\"alternate\" href=\"https://ex.com/1\"/></entry>\
        <entry><title>two</title><link rel=\"enclosure\" href=\"https://ex.com/2.mp3\"/></entry>\
        <entry><title>three</title><link href=\"https://ex.com/3\"/><link rel=\"alternate\" href=\"https://ex.com/no\"/></entry>\
        </feed>";
    let feed = parse(xml);
    assert_eq!(feed.title, "Bare");
    let links: Vec<&str> = feed.entries.iter().map(|e| e.link.as_str()).collect();
    assert_eq!(links, ["https://ex.com/1", "https://ex.com/2.mp3", "https://ex.com/3"]);
}

#[test]
fn atom_content_by_type() {
    let entry = |body: &str| {
        let xml = format!(
            "<feed xmlns=\"http://www.w3.org/2005/Atom\"><entry><link href=\"https://ex.com/a\"/>{body}</entry></feed>"
        );
        only(&xml)
    };
    // Escaped HTML, the usual way.
    let html = entry("<content type=\"html\">&lt;p&gt;a &amp;amp; b&lt;/p&gt;</content>");
    assert_eq!(
        (html.content.as_str(), html.summary.as_str()),
        ("<p>a &amp; b</p>", "a & b")
    );
    // Text is text: what looks like a tag in it is not one.
    let text = entry("<content type=\"text\">1 &lt; 2 &amp; &lt;b&gt;not bold&lt;/b&gt;</content>");
    assert_eq!(text.content, "1 &lt; 2 &amp; &lt;b&gt;not bold&lt;/b&gt;");
    assert_eq!(text.summary, "1 < 2 & <b>not bold</b>");
    // XHTML: the markup is the children. (feed.py reads none of it.)
    let xhtml = entry(
        "<content type=\"xhtml\"><div xmlns=\"http://www.w3.org/1999/xhtml\"><p>a &amp; <b>b</b></p><br/></div></content>",
    );
    assert_eq!(
        xhtml.content,
        "<div xmlns=\"http://www.w3.org/1999/xhtml\"><p>a &amp; <b>b</b></p><br/></div>"
    );
    assert_eq!(xhtml.summary, "a & b");
    // An empty content does not hide the summary.
    let summary = entry("<content type=\"html\"></content><summary>s</summary>");
    assert_eq!(summary.content, "s");
    assert_eq!(entry("").content, "");
}

#[test]
fn rss_one_point_zero() {
    let xml = "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns=\"http://purl.org/rss/1.0/\" \
        xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
        <channel><title>RDF feed</title><link>https://ex.com/</link></channel>\
        <item><title>one</title><link>https://ex.com/1</link><dc:date>2024-02-03T10:00:00Z</dc:date>\
        <dc:creator>Carol</dc:creator><description>d</description></item></rdf:RDF>";
    let feed = parse(xml);
    assert_eq!(feed.title, "RDF feed");
    let entry = &feed.entries[0];
    assert_eq!((entry.title.as_str(), entry.link.as_str()), ("one", "https://ex.com/1"));
    assert_eq!((entry.author.as_str(), entry.timestamp), ("Carol", Some(1_706_954_400)));
}

#[test]
fn elements_of_other_namespaces_are_not_the_feed_s() {
    let xml = "<rss xmlns:media=\"http://search.yahoo.com/mrss/\" xmlns:atom=\"http://www.w3.org/2005/Atom\" \
        xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\"><channel>\
        <atom:link rel=\"self\" href=\"https://ex.com/feed\"/><title>Podcast</title>\
        <item><media:title>media title</media:title><atom:link href=\"https://ex.com/atom\"/>\
        <itunes:author>itunes</itunes:author><media:description>media</media:description>\
        <title>real title</title><link>https://ex.com/real</link><description>real</description></item>\
        </channel></rss>";
    let feed = parse(xml);
    assert_eq!(feed.title, "Podcast");
    let entry = &feed.entries[0];
    assert_eq!(
        (
            entry.title.as_str(),
            entry.link.as_str(),
            entry.content.as_str(),
            entry.author.as_str()
        ),
        ("real title", "https://ex.com/real", "real", "")
    );
}

#[test]
fn text_is_read_as_element_tree_does() {
    // CDATA and text side by side are one text; whitespace around a link is not part of it.
    let xml = "<rss><channel><item><title>a <![CDATA[<b>]]> c</title>\
               <link>\n  https://ex.com/a\n</link><description>x<![CDATA[<p>y</p>]]>z</description></item></channel></rss>";
    let entry = only(xml);
    assert_eq!(entry.title, "a <b> c");
    assert_eq!(entry.link, "https://ex.com/a");
    assert_eq!(entry.id, "aHR0cHM6Ly9leC5jb20vYQ==");
    assert_eq!(entry.content, "x<p>y</p>z");
}

#[test]
fn links_ids_and_duplicates() {
    let xml = "<rss><channel>\
        <item><title>relative</title><link>/posts/1</link></item>\
        <item><title>guid only</title><guid>urn:uuid:1234</guid></item>\
        <item><title>nothing to tell it by</title><description>d</description></item>\
        <item><title>absolute</title><link>https://other.example/x</link></item>\
        <item><title>again</title><link>https://other.example/x</link></item>\
        </channel></rss>";
    // With no base, a relative link stays as it is.
    let feed = parse(xml);
    assert_eq!(titles(&feed), ["relative", "guid only", "absolute"]);
    assert_eq!(feed.entries[0].link, "/posts/1");
    assert_eq!(feed.entries[1].link, "");
    assert_eq!(feed.entries[1].id, "dXJuOnV1aWQ6MTIzNA=="); // base64 of urn:uuid:1234

    let feed = parse_feed(xml.as_bytes().to_vec(), Some("https://ex.com/blog/feed.xml".into())).unwrap();
    assert_eq!(feed.entries[0].link, "https://ex.com/posts/1");
    assert_eq!(feed.entries[2].link, "https://other.example/x");
}

#[test]
fn latin_1_feeds_are_read() {
    let mut xml = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><rss><channel><title>Caf\xe9</title>".to_vec();
    xml.extend(b"<item><title>\xc0 l'\xe9t\xe9</title><link>https://ex.com/a</link></item></channel></rss>");
    let feed = parse_feed(xml, None).unwrap();
    assert_eq!(feed.title, "Café");
    assert_eq!(titles(&feed), ["À l'été"]);
}

#[test]
fn what_is_not_a_feed() {
    let message = |xml: &str| match parse_feed(xml.as_bytes().to_vec(), None) {
        Err(error @ FeedError::NotAFeed { .. }) => error_message(error),
        other => panic!("{xml:?} gave {other:?}"),
    };
    let page = "not a feed: this is a web page: look on it for a link named RSS, Atom or feed";
    assert_eq!(
        message("<!DOCTYPE html><html><head><title>x</title></head><body><p>hi<br></body></html>"),
        page
    );
    assert_eq!(message("<html><body/></html>"), page);
    assert_eq!(
        message("<opml/>"),
        "not a feed: the document is a <opml>, neither RSS nor Atom"
    );
    assert!(message("").starts_with("not a feed: invalid XML"));
    assert!(message("{\"json\": true}").starts_with("not a feed: invalid XML"));
    assert!(message("<rss><channel><item></channel></rss>").starts_with("not a feed: invalid XML"));

    // An empty feed is a feed.
    assert_eq!(parse("<rss><channel><title>Empty</title></channel></rss>").entries, []);
}

#[test]
fn hostile_xml_is_refused_or_harmless() {
    // Entities that expand into one another ("billion laughs"): refused, and fast.
    let mut laughs = String::from("<?xml version=\"1.0\"?><!DOCTYPE lolz [<!ENTITY lol0 \"lol\">");
    for i in 1..10 {
        let previous = format!("&lol{};", i - 1).repeat(10);
        laughs.push_str(&format!("<!ENTITY lol{i} \"{previous}\">"));
    }
    laughs.push_str("]><rss><channel><item><title>&lol9;</title><link>https://ex.com/a</link></item></channel></rss>");
    let started = std::time::Instant::now();
    assert!(matches!(
        parse_feed(laughs.into_bytes(), None),
        Err(FeedError::NotAFeed { .. })
    ));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));

    // An external entity is never read.
    let xxe = "<?xml version=\"1.0\"?><!DOCTYPE rss [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]>\
               <rss><channel><item><title>&xxe;</title><link>https://ex.com/a</link></item></channel></rss>";
    match parse_feed(xxe.as_bytes().to_vec(), None) {
        Ok(feed) => assert!(!feed.entries[0].title.contains("root"), "{feed:?}"),
        Err(error) => assert!(matches!(error, FeedError::NotAFeed { .. })),
    }

    // A plain internal entity, as old feeds have, is fine.
    let dtd = "<?xml version=\"1.0\"?><!DOCTYPE rss [<!ENTITY nbsp \"&#160;\">]>\
               <rss><channel><item><title>a&nbsp;b</title><link>https://ex.com/a</link></item></channel></rss>";
    assert_eq!(titles(&parse(dtd)), ["a�b"]);
}

/// Parser and renderer together: what a feed can put in an article never reaches the page as markup.
#[test]
fn an_article_is_rendered_safely() {
    let entry = only(&make_rss(&[&[
        ("title", "<script>alert(1)</script> & co"),
        ("link", "https://ex.com/blog/post"),
        (
            "content_encoded",
            "<p onclick=\"alert(1)\">Hello <a href=\"/about\">us</a> <a href=\"javascript:alert(1)\">x</a></p>\
             <script>alert(2)</script><img src=\"pic.png\" alt=\"A pic\" onerror=\"alert(3)\"><iframe src=\"//evil\"></iframe>",
        ),
    ]]));
    let page = article_page(
        entry.title.clone(),
        "Test Feed · </p><script>".into(),
        entry.content.clone(),
        entry.link.clone(),
        false,
    );
    assert!(page.contains("<h1>&lt;script&gt;alert(1)&lt;/script&gt; &amp; co</h1>"));
    assert!(page.contains(
        "<p>Hello <a href=\"https://ex.com/about\" rel=\"noopener noreferrer\">us</a> <a>x</a></p><span class=\"image\">A pic</span>\n"
    ));
    for forbidden in [
        "<script",
        "alert(2)",
        "onclick",
        "onerror",
        "javascript:",
        "<iframe",
        "<img",
    ] {
        assert!(!page.contains(forbidden), "{forbidden} in {page}");
    }
    assert!(page.contains("img-src 'none'"));

    let page = article_page(entry.title, String::new(), entry.content, entry.link, true);
    assert!(page.contains("<img src=\"https://ex.com/blog/pic.png\" alt=\"A pic\" referrerpolicy=\"no-referrer\">"));
    assert!(page.contains("img-src https:;"));
    assert!(!page.contains("onerror"));
}

// ---- fetching: feed.py's tests/test_fetch.py, then redirects -------------------------------

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn feed(body: &str) -> Response {
    Response {
        status: 200,
        headers: vec![("Content-Type".into(), "application/rss+xml; charset=utf-8".into())],
        body: body.as_bytes().to_vec(),
    }
}

fn redirect(status: u16, location: &str) -> Response {
    Response {
        status,
        headers: vec![("Location".into(), location.into())],
        body: Vec::new(),
    }
}

/// What the server was asked: the path, and the headers with lowercase names.
#[derive(Debug, Clone)]
struct Request {
    path: String,
    headers: HashMap<String, String>,
}

/// feed.py's `FeedServer`, with a route per path: serves on a local port until the test ends
/// (its thread dies with the process), and keeps every request it got.
struct Server {
    port: u16,
    routes: Arc<Mutex<HashMap<String, Response>>>,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = Self {
            port: listener.local_addr().unwrap().port(),
            routes: Arc::default(),
            requests: Arc::default(),
        };
        let (routes, requests) = (server.routes.clone(), server.requests.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = Self::answer(stream, &routes, &requests);
            }
        });
        server
    }

    fn answer(
        mut stream: TcpStream,
        routes: &Mutex<HashMap<String, Response>>,
        requests: &Mutex<Vec<Request>>,
    ) -> std::io::Result<()> {
        let mut lines = BufReader::new(stream.try_clone()?).lines();
        let first = lines.next().transpose()?.unwrap_or_default();
        let path = first.split(' ').nth(1).unwrap_or("").to_string();
        let mut headers = HashMap::new();
        for line in lines {
            let line = line?;
            let Some((name, value)) = line.split_once(':') else {
                break;
            };
            headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
        }
        requests.lock().unwrap().push(Request {
            path: path.clone(),
            headers,
        });
        let routes = routes.lock().unwrap();
        let missing = Response {
            status: 404,
            headers: Vec::new(),
            body: Vec::new(),
        };
        let response = routes.get(&path).unwrap_or(&missing);
        write!(stream, "HTTP/1.1 {} X\r\nConnection: close\r\n", response.status)?;
        for (name, value) in &response.headers {
            write!(stream, "{name}: {value}\r\n")?;
        }
        write!(stream, "Content-Length: {}\r\n\r\n", response.body.len())?;
        stream.write_all(&response.body)
    }

    fn route(&self, path: &str, response: Response) -> &Self {
        self.routes.lock().unwrap().insert(path.into(), response);
        self
    }

    /// By IP address.
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    /// The same server, under another host name.
    fn url_by_name(&self, path: &str) -> String {
        format!("http://localhost:{}{path}", self.port)
    }

    fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    /// The path and the Cookie header of each request, in order.
    fn cookies(&self) -> Vec<(String, Option<String>)> {
        self.requests()
            .into_iter()
            .map(|r| (r.path, r.headers.get("cookie").cloned()))
            .collect()
    }
}

fn one_item(title: &str) -> String {
    make_rss(&[&[("title", title), ("link", "https://ex.com/x"), ("description", "d")]])
}

#[test]
fn fetches_and_parses_rss() {
    let server = Server::start();
    server.route(
        "/",
        feed(&make_rss(&[
            &[
                ("title", "A"),
                ("link", "https://ex.com/a"),
                ("pubDate", "Mon, 01 Jan 2024 12:00:00 +0000"),
                ("description", "first"),
            ],
            &[
                ("title", "B"),
                ("link", "https://ex.com/b"),
                ("pubDate", "Tue, 02 Jan 2024 12:00:00 +0000"),
                ("description", "second"),
            ],
        ])),
    );
    let fetched = fetch_feed(server.url("/"), None).unwrap();
    assert_eq!(titles(&fetched), ["A", "B"]);
    let request = &server.requests()[0];
    assert!(request.headers["user-agent"].starts_with("feed-core/"));
    assert_eq!(request.headers["host"], format!("127.0.0.1:{}", server.port));
}

#[test]
fn fetches_and_parses_atom() {
    let server = Server::start();
    let body = make_atom(&[&[
        ("title", "Atom A"),
        ("link", "/atom/a"),
        ("updated", "2024-02-03T10:00:00Z"),
        ("content", "body A"),
    ]]);
    server.route(
        "/feeds/atom.xml",
        Response {
            headers: vec![("Content-Type".into(), "application/atom+xml; charset=utf-8".into())],
            ..feed(&body)
        },
    );
    let fetched = fetch_feed(server.url("/feeds/atom.xml"), None).unwrap();
    assert_eq!(titles(&fetched), ["Atom A"]);
    // A relative link is resolved against where the feed was fetched.
    assert_eq!(fetched.entries[0].link, server.url("/atom/a"));
}

#[test]
fn cookies_are_sent_in_header() {
    let server = Server::start();
    server.route("/", feed(&one_item("t")));
    fetch_feed(server.url("/"), Some("session=abc; user=42".into())).unwrap();
    assert_eq!(
        server.cookies(),
        [("/".to_string(), Some("session=abc; user=42".to_string()))]
    );
}

#[test]
fn no_cookie_header_when_not_provided() {
    let server = Server::start();
    server.route("/", feed(&one_item("t")));
    fetch_feed(server.url("/"), None).unwrap();
    fetch_feed(server.url("/"), Some("  ".into())).unwrap();
    assert_eq!(server.cookies(), [("/".to_string(), None), ("/".to_string(), None)]);
}

#[test]
fn a_cookie_that_could_forge_a_header_is_refused_before_any_request() {
    let server = Server::start();
    server.route("/", feed(&one_item("t")));
    assert_eq!(
        fetch_feed(server.url("/"), Some("a=b\r\nX-Evil: 1".into())),
        Err(FeedError::InvalidCookies)
    );
    assert!(server.requests().is_empty());
}

#[test]
fn redirects_on_the_same_server_keep_the_cookies() {
    let server = Server::start();
    server
        .route("/old", redirect(301, "/moved/feed.xml"))
        .route("/moved/feed.xml", redirect(302, &server.url("/final?x=1")))
        .route("/final?x=1", feed(&one_item("arrived")));
    let fetched = fetch_feed(server.url("/old"), Some("k=v".into())).unwrap();
    assert_eq!(titles(&fetched), ["arrived"]);
    let cookie = Some("k=v".to_string());
    assert_eq!(
        server.cookies(),
        [
            ("/old".to_string(), cookie.clone()),
            ("/moved/feed.xml".to_string(), cookie.clone()),
            ("/final?x=1".to_string(), cookie),
        ]
    );
}

#[test]
fn a_redirect_to_another_host_is_followed_without_the_cookies() {
    // The same server under two names: for a client, 127.0.0.1 and localhost are two hosts.
    let server = Server::start();
    server
        .route("/feed", redirect(302, &server.url_by_name("/elsewhere")))
        .route("/elsewhere", redirect(307, &server.url("/back")))
        .route("/back", feed(&one_item("arrived")));
    let fetched = fetch_feed(server.url("/feed"), Some("session=secret".into())).unwrap();
    assert_eq!(titles(&fetched), ["arrived"]);
    let cookie = Some("session=secret".to_string());
    assert_eq!(
        server.cookies(),
        [
            ("/feed".to_string(), cookie.clone()),
            ("/elsewhere".to_string(), None),
            // Back on the feed's own server: they are its cookies.
            ("/back".to_string(), cookie),
        ]
    );
    let hosts: Vec<String> = server.requests().iter().map(|r| r.headers["host"].clone()).collect();
    assert!(hosts[1].starts_with("localhost:"), "{hosts:?}");
}

#[test]
fn a_redirect_to_another_port_is_followed_without_the_cookies() {
    let (first, second) = (Server::start(), Server::start());
    first.route("/feed", redirect(308, &second.url("/feed")));
    second.route("/feed", feed(&one_item("arrived")));
    let fetched = fetch_feed(first.url("/feed"), Some("session=secret".into())).unwrap();
    assert_eq!(titles(&fetched), ["arrived"]);
    assert_eq!(
        first.cookies(),
        [("/feed".to_string(), Some("session=secret".to_string()))]
    );
    assert_eq!(second.cookies(), [("/feed".to_string(), None)]);
    // No other header of the first request tells the second server about it.
    assert!(!second.requests()[0].headers.contains_key("referer"));
    assert!(!second.requests()[0].headers.contains_key("authorization"));
}

#[test]
fn redirects_end() {
    let server = Server::start();
    server.route("/loop", redirect(302, "/loop"));
    assert_eq!(fetch_feed(server.url("/loop"), None), Err(FeedError::TooManyRedirects));
    assert_eq!(server.requests().len(), 6, "the first request and 5 redirects");

    server
        .route("/to-file", redirect(302, "file:///etc/passwd"))
        .route("/to-ftp", redirect(302, "ftp://ex.com/feed"))
        .route("/nowhere", redirect(302, ""))
        .route(
            "/no-location",
            Response {
                status: 302,
                headers: Vec::new(),
                body: Vec::new(),
            },
        )
        .route(
            "/not-modified",
            Response {
                status: 304,
                headers: vec![("Location".into(), "/loop".into())],
                body: Vec::new(),
            },
        );
    assert!(matches!(
        fetch_feed(server.url("/to-file"), None),
        Err(FeedError::InvalidUrl { .. })
    ));
    assert!(matches!(
        fetch_feed(server.url("/to-ftp"), None),
        Err(FeedError::InvalidUrl { .. })
    ));
    assert_eq!(
        fetch_feed(server.url("/no-location"), None),
        Err(FeedError::Http { status: 302 })
    );
    assert_eq!(
        fetch_feed(server.url("/not-modified"), None),
        Err(FeedError::Http { status: 304 })
    );
    // An empty Location is the page itself: a loop like any other.
    assert_eq!(
        fetch_feed(server.url("/nowhere"), None),
        Err(FeedError::TooManyRedirects)
    );
}

#[test]
fn a_redirect_cannot_make_this_send_a_password() {
    let server = Server::start();
    let with_password = format!("http://user:pw@127.0.0.1:{}/feed", server.port);
    server
        .route("/start", redirect(302, &with_password))
        .route("/feed", feed(&one_item("arrived")));
    assert_eq!(titles(&fetch_feed(server.url("/start"), None).unwrap()), ["arrived"]);
    assert!(server
        .requests()
        .iter()
        .all(|r| !r.headers.contains_key("authorization")));
}

#[test]
fn errors_are_told_apart() {
    let server = Server::start();
    server
        .route(
            "/forbidden",
            Response {
                status: 403,
                headers: Vec::new(),
                body: b"no".to_vec(),
            },
        )
        .route("/page", feed("<!DOCTYPE html><html><body><p>Log in<br></body></html>"))
        .route(
            "/big",
            feed(&format!("<rss><!--{}--></rss>", "x".repeat(10 * 1024 * 1024))),
        );
    assert_eq!(
        fetch_feed(server.url("/forbidden"), None),
        Err(FeedError::Http { status: 403 })
    );
    assert_eq!(
        fetch_feed(server.url("/missing"), None),
        Err(FeedError::Http { status: 404 })
    );
    assert!(matches!(
        fetch_feed(server.url("/page"), None),
        Err(FeedError::NotAFeed { .. })
    ));
    assert_eq!(fetch_feed(server.url("/big"), None), Err(FeedError::TooLarge));
    assert!(matches!(
        fetch_feed("ftp://ex.com/feed".into(), None),
        Err(FeedError::InvalidUrl { .. })
    ));
    assert_eq!(
        error_message(FeedError::Http { status: 403 }),
        "the server answered 403"
    );

    // Nothing listens there (the port was just free).
    let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let gone = fetch_feed(format!("http://127.0.0.1:{free}/"), None);
    assert!(matches!(gone, Err(FeedError::Network { .. })), "{gone:?}");
}
