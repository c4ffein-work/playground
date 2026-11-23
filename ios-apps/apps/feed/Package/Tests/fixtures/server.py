#!/usr/bin/env python3
"""The feeds the FeedKit tests read: server.py PORT   (started by apps/feed/swift-test.sh)

Listens on 127.0.0.1 only. A test owns the feeds under a token of its own, and says
what they hold with a GET, so that it needs nothing but the client under test:

  /set/<token>?items=old:1,new:2    what /feed/<token>.xml serves from now on: an item per
                                    title:day (its date is that day of January 2024, none for day 0)
  /set/<token>?status=500           ... or the status it fails with
  /feed/<token>.xml                 an RSS feed named "Feed <token>" (404 until it is set)
  /atom.xml                         an Atom feed with one entry
  /hostile.xml                      an article whose HTML tries what a web page must not do
  /private.xml                      403 without the cookie session=abc
  /echo.xml                         one article, whose title is the Cookie header it was asked with
  /redirect/same                    302 to /echo.xml on this host
  /redirect/other                   302 to /echo.xml on another host (the same server, as localhost)
  /page.html                        a web page
"""

import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit
from xml.sax.saxutils import escape

PORT = int(sys.argv[1])
feeds = {}  # token -> (status, [(title, day)])
lock = threading.Lock()


def rss(title, items):
    """items: (title, link, pubDate or None, HTML)"""
    xml = [f'<?xml version="1.0" encoding="UTF-8"?><rss version="2.0"><channel><title>{escape(title)}</title>']
    for item_title, link, date, html in items:
        xml.append(f"<item><title>{escape(item_title)}</title><link>{escape(link)}</link>")
        if date:
            xml.append(f"<pubDate>{date}</pubDate>")
        xml.append(f"<description>{escape(html)}</description></item>")
    xml.append("</channel></rss>")
    return "".join(xml)


ATOM = """<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>Atom fixture</title><id>urn:fixture</id>
<entry><title>Atom entry</title><link rel="alternate" href="https://ex.com/atom/1"/>
<published>2024-02-03T10:00:00Z</published><author><name>Bob</name></author>
<content type="html">&lt;p&gt;atom body&lt;/p&gt;</content></entry></feed>"""

HOSTILE = rss(
    "Hostile",
    [
        (
            "<script>alert(0)</script>",
            "https://ex.com/blog/post",
            "Mon, 01 Jan 2024 12:00:00 +0000",
            '<p onclick="alert(1)">Hello <a href="/about">us</a> <a href="javascript:alert(2)">x</a></p>'
            '<script>alert(3)</script><img src="pic.png" alt="A pic" onerror="alert(4)">'
            '<iframe src="https://evil.example/"></iframe><style>body{display:none}</style>',
        ),
        ("Not a web link", "javascript:alert(5)", None, "<p>x</p>"),
    ],
)


class Handler(BaseHTTPRequestHandler):
    def answer(self, status, body="", content_type="application/rss+xml; charset=utf-8", location=None):
        body = body.encode()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        if location:
            self.send_header("Location", location)
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        url = urlsplit(self.path)
        parts = url.path.strip("/").split("/")
        cookie = self.headers.get("Cookie")
        if parts[0] == "set" and len(parts) == 2:
            query = parse_qs(url.query)
            items = [i.split(":") for i in query.get("items", [""])[0].split(",") if i]
            with lock:
                feeds[parts[1]] = (int(query.get("status", ["200"])[0]), [(t, int(d)) for t, d in items])
            self.answer(200, rss("set", []))
        elif parts[0] == "feed" and len(parts) == 2 and parts[1].endswith(".xml"):
            token = parts[1][: -len(".xml")]
            with lock:
                status, items = feeds.get(token, (404, []))
            if status != 200:
                return self.answer(status)
            self.answer(
                200,
                rss(
                    f"Feed {token}",
                    [
                        (
                            title,
                            f"https://ex.com/{token}/{title}",
                            f"{day:02d} Jan 2024 12:00:00 +0000" if day else None,
                            f"<p>About <b>{title}</b></p>",
                        )
                        for title, day in items
                    ],
                ),
            )
        elif url.path == "/atom.xml":
            self.answer(200, ATOM, "application/atom+xml; charset=utf-8")
        elif url.path == "/hostile.xml":
            self.answer(200, HOSTILE)
        elif url.path == "/private.xml":
            if cookie == "session=abc":
                self.answer(200, rss("Private", [("members only", "https://ex.com/private/1", None, "<p>secret</p>")]))
            else:
                self.answer(403)
        elif url.path == "/echo.xml":
            self.answer(200, rss("Echo", [(f"cookie: {cookie or 'none'}", "https://ex.com/echo", None, "")]))
        elif url.path == "/redirect/same":
            self.answer(302, location="/echo.xml")
        elif url.path == "/redirect/other":
            self.answer(302, location=f"http://localhost:{PORT}/echo.xml")
        elif url.path == "/page.html":
            self.answer(200, "<!DOCTYPE html><html><body><p>A page<br></body></html>", "text/html")
        else:
            self.answer(404)

    def log_message(self, *args):
        pass


ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
