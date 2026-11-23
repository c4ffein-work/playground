//! What is done to the HTML of an article before anything shows it. It comes from
//! whoever wrote the feed, so it is never passed on: it is read tag by tag and
//! written again from an allowlist.
//!
//! Why the output is safe to give to a web view, whatever the input:
//! - text is written with `<` and `>` escaped, so the only tags in the output are
//!   the ones written here;
//! - those are a fixed list of names, with no attribute but `href` on `a` and
//!   `src`/`alt` on `img`, and no element whose content is not parsed as text
//!   (`script`, `style`, `textarea`, `title`...);
//! - a URL is parsed, resolved, checked to be http(s) and written again from its
//!   parsed form, quoted and escaped.
//!
//! So it does not matter whether this reads broken markup the way a browser would.
//! [`page`] adds a Content-Security-Policy on top, and the app turns JavaScript off.

use url::Url;

/// Removed with everything inside.
const DROPPED_WITH_CONTENT: [&str; 17] = [
    "script", "style", "iframe", "object", "svg", "math", "template", "applet", "head", "title", "textarea", "select",
    "button", "canvas", "audio", "video", "frameset",
];

/// Written back as they are, with no attribute.
const KEPT: [&str; 47] = [
    "p",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "strong",
    "b",
    "em",
    "i",
    "u",
    "s",
    "del",
    "ins",
    "sub",
    "sup",
    "small",
    "mark",
    "code",
    "pre",
    "kbd",
    "samp",
    "var",
    "blockquote",
    "q",
    "cite",
    "abbr",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "figure",
    "figcaption",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "th",
    "td",
    "caption",
    "div",
    "span",
    "time",
];

/// Blocks with a meaning the reader has no use for: kept as blocks.
const KEPT_AS_DIV: [&str; 11] = [
    "section", "article", "header", "footer", "main", "nav", "aside", "center", "address", "form", "fieldset",
];

/// What does not separate two words.
const INLINE: [&str; 23] = [
    "a", "b", "strong", "em", "i", "u", "s", "del", "ins", "sub", "sup", "small", "mark", "code", "kbd", "samp", "var",
    "span", "q", "cite", "abbr", "time", "font",
];

/// The named references worth knowing in feeds; the others are left as written.
const ENTITIES: [(&str, char); 30] = [
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("nbsp", '\u{a0}'),
    ("hellip", '…'),
    ("mdash", '—'),
    ("ndash", '–'),
    ("lsquo", '‘'),
    ("rsquo", '’'),
    ("ldquo", '“'),
    ("rdquo", '”'),
    ("laquo", '«'),
    ("raquo", '»'),
    ("copy", '©'),
    ("reg", '®'),
    ("trade", '™'),
    ("euro", '€'),
    ("deg", '°'),
    ("middot", '·'),
    ("bull", '•'),
    ("times", '×'),
    ("eacute", 'é'),
    ("egrave", 'è'),
    ("ecirc", 'ê'),
    ("agrave", 'à'),
    ("ccedil", 'ç'),
    ("ugrave", 'ù'),
    ("ocirc", 'ô'),
];

struct Tag<'a> {
    /// Lowercase.
    name: String,
    closing: bool,
    /// Lowercase names, values as written (references not decoded).
    attrs: Vec<(String, &'a str)>,
}

impl Tag<'_> {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, value)| *value)
    }
}

enum Token<'a> {
    /// As written: references not decoded, and it may be a lone `<`.
    Text(&'a str),
    Tag(Tag<'a>),
}

/// Reads the tag at the start of `text` (which starts with `<`): the tag and its length.
/// `None` when this `<` starts no tag. A tag that is never closed runs to the end of the text.
fn parse_tag(text: &str) -> Option<(Tag<'_>, usize)> {
    let bytes = text.as_bytes();
    let space = |i: usize| bytes[i].is_ascii_whitespace();
    let closing = bytes.get(1) == Some(&b'/');
    let mut i = if closing { 2 } else { 1 };
    if !bytes.get(i)?.is_ascii_alphabetic() {
        return None;
    }
    let start = i;
    while i < bytes.len() && !(space(i) || matches!(bytes[i], b'/' | b'>')) {
        i += 1;
    }
    let name = text[start..i].to_ascii_lowercase();
    let mut attrs = Vec::new();
    loop {
        while i < bytes.len() && (space(i) || bytes[i] == b'/') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'>' {
            i += 1;
            break;
        }
        // A name is at least one character, whatever it is: this always moves forward.
        let start = i;
        i += 1;
        while i < bytes.len() && !(space(i) || matches!(bytes[i], b'=' | b'>' | b'/')) {
            i += 1;
        }
        let attr = text[start..i].to_ascii_lowercase();
        while i < bytes.len() && space(i) {
            i += 1;
        }
        let mut value = "";
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1;
            while i < bytes.len() && space(i) {
                i += 1;
            }
            if i < bytes.len() && matches!(bytes[i], b'"' | b'\'') {
                let quote = bytes[i];
                let start = i + 1;
                i = start;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                value = &text[start..i];
                i = (i + 1).min(bytes.len());
            } else {
                let start = i;
                while i < bytes.len() && !(space(i) || bytes[i] == b'>') {
                    i += 1;
                }
                value = &text[start..i];
            }
        }
        attrs.push((attr, value));
    }
    Some((Tag { name, closing, attrs }, i))
}

/// Where the element `name`, opened just before `from`, ends: after its end tag, or at the end of the text.
fn end_of_element(html: &str, from: usize, name: &str) -> usize {
    let bytes = html.as_bytes();
    let mut at = from;
    while let Some(found) = html[at..].find("</") {
        let start = at + found + 2;
        let end = start + name.len();
        let is_it = bytes
            .get(start..end)
            .is_some_and(|b| b.eq_ignore_ascii_case(name.as_bytes()))
            && !bytes.get(end).is_some_and(|b| b.is_ascii_alphanumeric());
        if is_it {
            return html[end..].find('>').map_or(html.len(), |gt| end + gt + 1);
        }
        at = start;
    }
    html.len()
}

/// Calls `visit` with every piece of text and every tag, in order. Comments, declarations
/// and the elements of [`DROPPED_WITH_CONTENT`] are skipped here, for every user.
fn walk<'a>(html: &'a str, mut visit: impl FnMut(Token<'a>)) {
    let mut i = 0;
    while i < html.len() {
        let Some(lt) = html[i..].find('<').map(|found| i + found) else {
            visit(Token::Text(&html[i..]));
            break;
        };
        if lt > i {
            visit(Token::Text(&html[i..lt]));
        }
        let rest = &html[lt..];
        if rest.starts_with("<!--") {
            i = rest.find("-->").map_or(html.len(), |end| lt + end + 3);
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            i = rest.find('>').map_or(html.len(), |end| lt + end + 1);
        } else if let Some((tag, length)) = parse_tag(rest) {
            i = lt + length;
            if !DROPPED_WITH_CONTENT.contains(&tag.name.as_str()) {
                visit(Token::Tag(tag));
            } else if !tag.closing {
                i = end_of_element(html, i, &tag.name);
            }
        } else {
            visit(Token::Text("<"));
            i = lt + 1;
        }
    }
}

/// Decodes `&#233;`, `&#xe9;` and the references of [`ENTITIES`]. Anything else stays as written.
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        // The longest reference HTML has is 33 characters long.
        let decoded = rest[1..]
            .bytes()
            .take(34)
            .position(|b| b == b';')
            .and_then(|semi| Some((semi, decode_reference(&rest[1..1 + semi])?)));
        match decoded {
            Some((semi, c)) => {
                out.push(c);
                rest = &rest[semi + 2..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `name` is what is between `&` and `;`.
fn decode_reference(name: &str) -> Option<char> {
    if let Some(number) = name.strip_prefix('#') {
        let code = match number.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => number.parse().ok()?,
        };
        // Not a character (0, a surrogate, too big): the replacement character, as a browser does.
        return Some(char::from_u32(code).filter(|c| *c != '\0').unwrap_or('\u{fffd}'));
    }
    ENTITIES.iter().find(|(n, _)| *n == name).map(|(_, c)| *c)
}

/// Text, for where HTML is expected: every character that means something is escaped.
/// Also right inside a quoted attribute.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// The absolute http(s) URL an attribute points to, resolved against `base`. `None` for anything else
/// (`javascript:`, `data:`, `file:`, a relative URL with no base...).
fn resolve(value: &str, base: Option<&Url>) -> Option<Url> {
    let value = decode_entities(value);
    let url = match base {
        Some(base) => base.join(value.trim()),
        None => Url::parse(value.trim()),
    }
    .ok()?;
    (matches!(url.scheme(), "http" | "https") && url.has_host()).then_some(url)
}

/// `width="1"`: the size of an image that is only there to tell who reads.
fn is_pixel(tag: &Tag) -> bool {
    ["width", "height"].iter().any(|name| {
        let size = tag.attr(name).unwrap_or("").trim().trim_end_matches("px");
        matches!(size, "0" | "1")
    })
}

/// Rewrites untrusted HTML into a fragment that is safe to put in [`page`]: see the top of this file.
/// Relative links are resolved against `base`. With `images`, an https image is kept; without,
/// or over plain http, its description is shown in its place and nothing is loaded.
pub fn sanitize(html: &str, base: Option<&Url>, images: bool) -> String {
    let mut out = String::with_capacity(html.len());
    walk(html, |token| match token {
        // `&` stays: a reference is still one, and a stray `&` is text for a browser too.
        Token::Text(text) => out.push_str(&text.replace('<', "&lt;").replace('>', "&gt;")),
        Token::Tag(tag) => {
            let slash = if tag.closing { "/" } else { "" };
            let name = tag.name.as_str();
            match name {
                "br" | "hr" if !tag.closing => out.push_str(&format!("<{name}>")),
                "a" if tag.closing => out.push_str("</a>"),
                "a" => match tag.attr("href").and_then(|href| resolve(href, base)) {
                    Some(url) => out.push_str(&format!(
                        "<a href=\"{}\" rel=\"noopener noreferrer\">",
                        escape(url.as_str())
                    )),
                    None => out.push_str("<a>"),
                },
                "img" if !tag.closing => {
                    let alt = decode_entities(tag.attr("alt").unwrap_or(""));
                    let alt = alt.trim();
                    let source = tag.attr("src").and_then(|src| resolve(src, base));
                    match source {
                        _ if is_pixel(&tag) => {}
                        Some(url) if images && url.scheme() == "https" => out.push_str(&format!(
                            "<img src=\"{}\" alt=\"{}\" referrerpolicy=\"no-referrer\">",
                            escape(url.as_str()),
                            escape(alt)
                        )),
                        _ => out.push_str(&format!(
                            "<span class=\"image\">{}</span>",
                            escape(if alt.is_empty() { "image" } else { alt })
                        )),
                    }
                }
                _ if KEPT.contains(&name) => out.push_str(&format!("<{slash}{name}>")),
                _ if KEPT_AS_DIV.contains(&name) => out.push_str(&format!("<{slash}div>")),
                _ => {}
            }
        }
    });
    out
}

/// The text of some HTML on one line: no tag, references decoded, whitespace collapsed.
pub fn to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    walk(html, |token| match token {
        Token::Text(piece) => text.push_str(&decode_entities(piece)),
        // Two blocks, two cells, two lines: not one word.
        Token::Tag(tag) if !INLINE.contains(&tag.name.as_str()) => text.push(' '),
        Token::Tag(_) => {}
    });
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first `max` characters of the text of some HTML, for a list.
pub fn excerpt(html: &str, max: usize) -> String {
    let text = to_text(html);
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", text[..end].trim_end()),
        None => text,
    }
}

const STYLE: &str = "\
:root { color-scheme: light dark; }
body { font: -apple-system-body; font-family: -apple-system, system-ui, sans-serif; line-height: 1.5;
  margin: 16px; overflow-wrap: break-word; -webkit-text-size-adjust: 100%; }
header h1 { font-size: 1.4em; line-height: 1.25; margin: 0 0 0.3em; }
header p { margin: 0 0 1.5em; opacity: 0.6; font-size: 0.85em; }
h1, h2, h3, h4, h5, h6 { line-height: 1.25; }
img { max-width: 100%; height: auto; }
pre { overflow-x: auto; }
pre, code, kbd, samp { font-family: ui-monospace, Menlo, monospace; font-size: 0.9em; }
blockquote { margin: 1em 0; padding-left: 1em; border-left: 3px solid; opacity: 0.8; }
table { display: block; overflow-x: auto; border-collapse: collapse; }
th, td { padding: 0.2em 0.6em; border: 1px solid; }
.image { opacity: 0.6; font-style: italic; }
.image::before { content: '['; }
.image::after { content: ']'; }
";

/// A whole document for a web view: `title` and `byline` are text, `body` is the output of [`sanitize`].
/// Its policy lets nothing load (no script, frame, font, form, stylesheet), but https images when `images` is set.
pub fn page(title: &str, byline: &str, body: &str, images: bool) -> String {
    let images = if images { "https:" } else { "'none'" };
    format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; \
         img-src {images}; base-uri 'none'; form-action 'none'\">\n\
         <meta name=\"referrer\" content=\"no-referrer\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <style>\n{STYLE}</style></head>\n\
         <body><header><h1>{}</h1><p>{}</p></header>\n{body}\n</body></html>\n",
        escape(title),
        escape(byline),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(html: &str) -> String {
        sanitize(html, Some(&Url::parse("https://ex.com/blog/post").unwrap()), true)
    }

    #[test]
    fn formatting_is_kept() {
        assert_eq!(
            clean("<p>one <b>two</b> <em>three</em><br/>four</p><ul><li>five</li></ul>"),
            "<p>one <b>two</b> <em>three</em><br>four</p><ul><li>five</li></ul>"
        );
        assert_eq!(
            clean("<H2 CLASS=x>t</H2><pre><code>a &lt; b</code></pre>"),
            "<h2>t</h2><pre><code>a &lt; b</code></pre>"
        );
        assert_eq!(
            clean("<section><table><tr><td colspan=2>x</td></tr></table></section>"),
            "<div><table><tr><td>x</td></tr></table></div>"
        );
        assert_eq!(
            clean("plain &amp; simple, 1 < 2 > 0 & so on"),
            "plain &amp; simple, 1 &lt; 2 &gt; 0 & so on"
        );
        assert_eq!(clean("été <b>日本</b> 🎉"), "été <b>日本</b> 🎉");
        assert_eq!(clean(""), "");
    }

    #[test]
    fn scripts_and_friends_are_removed_with_their_content() {
        // feed.py's test_strips_script_and_style
        assert_eq!(
            clean("<p>keep</p><script>alert(1)</script><style>x{}</style>"),
            "<p>keep</p>"
        );
        assert_eq!(clean("a<SCRIPT type='x'>if (a</b>) alert(1)</SCRIPT >b"), "ab");
        assert_eq!(clean("a<iframe src=//evil></iframe>b<object data=x>c</object>d"), "abd");
        assert_eq!(clean("a<svg><script>alert(1)</script></svg>b"), "ab");
        assert_eq!(
            clean("a<math><mi>x</mi></math>b<template><img src=x></template>c"),
            "abc"
        );
        assert_eq!(clean("a<textarea></p><script>x</script></textarea>b"), "ab");
        assert_eq!(clean("a<video src=x><source src=y></video>b"), "ab");
        // Never closed: nothing after it is shown.
        assert_eq!(clean("a<script>alert(1)"), "a");
        assert_eq!(clean("a<style>p{}</styleX>b"), "a");
        // A closing tag alone opens nothing.
        assert_eq!(clean("a</script>b"), "ab");
    }

    #[test]
    fn no_tag_and_no_attribute_gets_through() {
        for (html, expected) in [
            (
                "<p onclick=\"alert(1)\" style=\"position:fixed\" id=x class=y>t</p>",
                "<p>t</p>",
            ),
            ("<b onmouseover=alert(1)>t</b>", "<b>t</b>"),
            ("<div data-x='>'>t</div>", "<div>t</div>"),
            ("<p title=\"a>b\" onclick='x'>t</p>", "<p>t</p>"),
            (
                "<form action=//evil><input name=a><button>go</button></form>",
                "<div></div>",
            ),
            (
                "<base href=//evil><link rel=stylesheet href=x><meta http-equiv=refresh content=0>t",
                "t",
            ),
            ("<embed src=x>t<body onload=alert(1)>u<html>v", "tuv"),
            ("<marquee>t</marquee><blink>u</blink><font color=red>v</font>", "tuv"),
            ("<o:p>word</o:p><svg:script>t</svg:script>", "wordt"),
            (
                "<!-- <script>alert(1)</script> -->t<!-->u<!doctype html>v<?php x ?>w",
                "tuvw",
            ),
            ("t<!-- never closed <b>", "t"),
            // As a browser reads HTML: a declaration up to the first `>`. What follows is text.
            ("<![CDATA[<script>alert(1)</script>]]>", "alert(1)]]&gt;"),
            // A tag that never ends takes the rest with it.
            ("t<b onclick=\"alert(1)", "t<b>"),
            (
                "t<img src='https://ex.com/a.png",
                "t<img src=\"https://ex.com/a.png\" alt=\"\" referrerpolicy=\"no-referrer\">",
            ),
            // Not tags: text.
            (
                "a < b, 1<2, <3, < script>, <>, </>, <",
                "a &lt; b, 1&lt;2, &lt;3, &lt; script&gt;, &lt;&gt;, &lt;/&gt;, &lt;",
            ),
            // What a parser confused by quotes or slashes could let through.
            ("<p/onclick=alert(1)>t", "<p>t"),
            ("<p \"><script>alert(1)</script>\">t", "<p>\"&gt;t"),
            (
                "<a href=\"x\"onclick=\"alert(1)\">t</a>",
                "<a href=\"https://ex.com/blog/x\" rel=\"noopener noreferrer\">t</a>",
            ),
            ("<scr<script>ipt>alert(1)</scr</script>ipt>", "ipt&gt;alert(1)ipt&gt;"),
            ("<<script>alert(1)</script>", "&lt;"),
            (
                "<img src=x onerror=alert(1)//>",
                "<img src=\"https://ex.com/blog/x\" alt=\"\" referrerpolicy=\"no-referrer\">",
            ),
        ] {
            assert_eq!(clean(html), expected, "{html}");
        }
    }

    /// Whatever goes in, the output holds no tag and no attribute but the ones written here.
    #[test]
    fn the_output_is_only_what_the_sanitizer_writes() {
        let pieces = [
            "<",
            ">",
            "/",
            "\"",
            "'",
            "=",
            " ",
            "!--",
            "-->",
            "script",
            "img",
            "a",
            "p",
            "src",
            "href",
            "onerror",
            "javascript:alert(1)",
            "&#x3c;",
            "&lt;",
            "é",
            "\n",
            "x",
            "<script>",
            "</script>",
            "<a href=",
            "<img src=",
            "<![CDATA[",
            "]]>",
            "<svg",
            "</svg>",
            "style",
        ];
        // A fixed pseudo-random sequence: the same inputs on every run.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = |bound: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % bound as u64) as usize
        };
        for _ in 0..20_000 {
            let input: String = (0..next(12) + 1).map(|_| pieces[next(pieces.len())]).collect();
            let output = clean(&input);
            let mut rest = output.as_str();
            while let Some(lt) = rest.find('<') {
                let gt = lt
                    + rest[lt..]
                        .find('>')
                        .unwrap_or_else(|| panic!("{input:?} -> {output:?}"));
                let tag = &rest[lt + 1..gt];
                let ok = match tag.split_once(' ') {
                    None => {
                        let name = tag.trim_start_matches('/');
                        KEPT.contains(&name) || matches!(name, "a" | "br" | "hr")
                    }
                    Some(("a", attrs)) => {
                        let url = attrs
                            .strip_prefix("href=\"")
                            .and_then(|a| a.strip_suffix("\" rel=\"noopener noreferrer\""));
                        url.is_some_and(|u| u.starts_with("https://") && !u.contains(['"', '<', '\'']))
                    }
                    Some(("img", attrs)) => {
                        let inner = attrs
                            .strip_prefix("src=\"https://")
                            .and_then(|a| a.strip_suffix("\" referrerpolicy=\"no-referrer\""));
                        // src="…" alt="…": exactly the two quotes in between.
                        inner
                            .is_some_and(|i| i.matches('"').count() == 2 && i.contains("\" alt=\"") && !i.contains('<'))
                    }
                    Some(("span", "class=\"image\"")) => true,
                    _ => false,
                };
                assert!(ok, "{input:?} -> {output:?}");
                rest = &rest[gt + 1..];
            }
            assert!(!rest.contains('>'), "{input:?} -> {output:?}");
        }
    }

    #[test]
    fn links_are_http_or_nothing() {
        let link = |href: &str| format!("<a href=\"{href}\" rel=\"noopener noreferrer\">t</a>");
        for (html, expected) in [
            (
                "<a href='https://other.example/a?b=1&amp;c=2#d'>t</a>",
                link("https://other.example/a?b=1&amp;c=2#d"),
            ),
            ("<a href=http://other.example>t</a>", link("http://other.example/")),
            (
                "<a href=\"/about\" target=_blank onclick=x>t</a>",
                link("https://ex.com/about"),
            ),
            ("<a href=next>t</a>", link("https://ex.com/blog/next")),
            ("<a href=//cdn.example/x>t</a>", link("https://cdn.example/x")),
            ("<a href=\"  https://ex.com/ a\n\">t</a>", link("https://ex.com/%20a")),
            (
                "<a href='https://ex.com/\"><script>alert(1)</script>'>t</a>",
                link("https://ex.com/%22%3E%3Cscript%3Ealert(1)%3C/script%3E"),
            ),
            ("<a href=\"javascript:alert(1)\">t</a>", "<a>t</a>".into()),
            ("<a href=\"JaVaScRiPt:alert(1)\">t</a>", "<a>t</a>".into()),
            ("<a href=\" javascript:alert(1)\">t</a>", "<a>t</a>".into()),
            ("<a href=\"java\tscript:alert(1)\">t</a>", "<a>t</a>".into()),
            ("<a href=\"java&#x73;cript:alert(1)\">t</a>", "<a>t</a>".into()),
            ("<a href=\"&#106;avascript&#58;alert(1)\">t</a>", "<a>t</a>".into()),
            (
                "<a href=\"data:text/html,<script>alert(1)</script>\">t</a>",
                "<a>t</a>".into(),
            ),
            ("<a href=\"vbscript:x\">t</a>", "<a>t</a>".into()),
            ("<a href=\"file:///etc/passwd\">t</a>", "<a>t</a>".into()),
            ("<a href=\"tel:112\">t</a>", "<a>t</a>".into()),
            ("<a href=\"myapp://do/something\">t</a>", "<a>t</a>".into()),
            ("<a name=x>t</a>", "<a>t</a>".into()),
            // A reference this does not know is not one for the browser either, once written back.
            (
                "<a href=\"java&Tab;script:alert(1)\">t</a>",
                link("https://ex.com/blog/java&amp;Tab;script:alert(1)"),
            ),
        ] {
            assert_eq!(clean(html), expected, "{html}");
        }
        // With no base, a relative link goes nowhere.
        assert_eq!(sanitize("<a href=/about>t</a>", None, true), "<a>t</a>");
        assert_eq!(
            sanitize("<a href=https://ex.com/about>t</a>", None, true),
            link("https://ex.com/about")
        );
    }

    #[test]
    fn images_are_https_or_their_description() {
        let image = |src: &str, alt: &str| format!("<img src=\"{src}\" alt=\"{alt}\" referrerpolicy=\"no-referrer\">");
        let html = "<img alt=\"a &quot;diagram&quot; <1>\" src=\"/i/x.png\" srcset=\"evil 2x\" onerror=\"alert(1)\">";
        assert_eq!(
            clean(html),
            image("https://ex.com/i/x.png", "a &quot;diagram&quot; &lt;1&gt;")
        );
        assert_eq!(
            clean("<img src=https://cdn.example/x.png>"),
            image("https://cdn.example/x.png", "")
        );

        let placeholder = |alt: &str| format!("<span class=\"image\">{alt}</span>");
        // feed.py's test_image_alt_rendered
        let base = Url::parse("https://ex.com/").unwrap();
        assert_eq!(
            sanitize("<img alt=\"diagram\" src=\"x.png\">", Some(&base), false),
            placeholder("diagram")
        );
        assert_eq!(
            sanitize("<img src=\"x.png\">", Some(&base), false),
            placeholder("image")
        );
        // Never over plain http, never anything else.
        assert_eq!(clean("<img src=http://ex.com/x.png alt=x>"), placeholder("x"));
        assert_eq!(clean("<img src=\"javascript:alert(1)\">"), placeholder("image"));
        assert_eq!(
            clean("<img src=\"data:image/svg+xml,<svg onload=alert(1)>\">"),
            placeholder("image")
        );
        assert_eq!(clean("<img alt=x>"), placeholder("x"));
        // A tracking pixel is not even mentioned.
        assert_eq!(clean("a<img src=https://t.example/p.gif width=1 height=1>b"), "ab");
        assert_eq!(clean("a<img src=https://t.example/p.gif height=\"0px\">b"), "ab");
        assert_eq!(
            sanitize("a<img src=https://t.example/p.gif width=1>b", None, false),
            "ab"
        );
    }

    #[test]
    fn references_are_decoded() {
        assert_eq!(
            decode_entities("a &amp; b &lt;c&gt; &quot;d&quot; &#233;&#xe9;&#XE9; &hellip;"),
            "a & b <c> \"d\" ééé …"
        );
        assert_eq!(
            decode_entities("&amp;lt; & &; &nope; &#; &#x; &#xzz; &amp"),
            "&lt; & &; &nope; &#; &#x; &#xzz; &amp"
        );
        assert_eq!(
            decode_entities("&#0;&#xd800;&#x110000;&#99999999999;"),
            "\u{fffd}\u{fffd}\u{fffd}&#99999999999;"
        );
        assert_eq!(decode_entities("é&amp;é&"), "é&é&");
        assert_eq!(
            decode_entities(&format!("&{};", "é".repeat(40))),
            format!("&{};", "é".repeat(40))
        );
    }

    #[test]
    fn text_of_html() {
        // feed.py's test_renders_lists and test_unescapes_entities
        assert_eq!(to_text("<ul><li>one</li><li>two</li></ul>"), "one two");
        assert_eq!(to_text("<p>a &amp; b &lt;c&gt; tail</p>"), "a & b <c> tail");
        assert_eq!(
            to_text("<p>keep</p><script>alert(1)</script><style>x{}</style>"),
            "keep"
        );
        assert_eq!(
            to_text("  <p>one<br>two</p>\n\n<p>th<b>r</b>ee <a href=x>four</a>.</p>"),
            "one two three four."
        );
        assert_eq!(to_text(""), "");
        assert_eq!(excerpt("<p>one two three</p>", 100), "one two three");
        assert_eq!(excerpt("<p>one two three</p>", 8), "one two…");
        assert_eq!(excerpt("ééééé", 3), "ééé…");
        assert_eq!(excerpt("ééé", 3), "ééé");
    }

    #[test]
    fn the_page_is_locked_down() {
        let html = page(
            "<b>T</b> & \"t\"",
            "feed · </p><script>x</script>",
            "<p>body</p>",
            false,
        );
        assert!(html.contains("<h1>&lt;b&gt;T&lt;/b&gt; &amp; &quot;t&quot;</h1>"));
        assert!(html.contains("<p>feed · &lt;/p&gt;&lt;script&gt;x&lt;/script&gt;</p>"));
        assert!(html.contains("<p>body</p>"));
        assert!(!html.contains("<script"));
        assert!(html.contains(
            "default-src 'none'; style-src 'unsafe-inline'; img-src 'none'; base-uri 'none'; form-action 'none'"
        ));
        // The policy comes before anything that could load.
        assert!(html.find("Content-Security-Policy").unwrap() < html.find("<style>").unwrap());
        assert!(page("t", "b", "", true).contains("img-src https:;"));
    }
}
