//! Static file serving for routes that have a `ROOT` instead of an upstream.
//!
//! Small files are read into memory once, at startup, and served from there
//! for the life of the process: a snapshot, never re-checked. Anything larger
//! than the route's limit, anything added later, and everything when the limit
//! is zero, is streamed from disk on each request.
//!
//! There are no directory listings. Dotfiles are never served (so a stray
//! `.env` or `.git` cannot leak), with the exception of `.well-known`.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::{Bytes, BytesMut};
use log::info;
use pingora::http::ResponseHeader;
use pingora::prelude::*;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::certs::BoxError;
use crate::config::StaticSite;
use crate::proxy::respond;

const INDEX: &str = "index.html";
const CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Meta {
    len: u64,
    /// Modification time, in whole seconds since the Unix epoch.
    modified: u64,
    content_type: &'static str,
}

impl Meta {
    fn new(path: &Path, metadata: &std::fs::Metadata) -> Meta {
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        Meta {
            len: metadata.len(),
            modified,
            content_type: content_type(path),
        }
    }

    /// Same recipe as nginx: cheap, and identical whether the file is served
    /// from memory or from disk.
    fn etag(&self) -> String {
        format!("\"{:x}-{:x}\"", self.modified, self.len)
    }

    fn last_modified(&self) -> String {
        httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(self.modified))
    }
}

struct MemFile {
    meta: Meta,
    body: Bytes,
}

enum Found {
    Memory(Arc<MemFile>),
    Disk(tokio::fs::File, Meta),
}

impl Found {
    fn meta(&self) -> Meta {
        match self {
            Found::Memory(file) => file.meta,
            Found::Disk(_, meta) => *meta,
        }
    }
}

enum Lookup {
    File(Found),
    /// A directory with an index, asked for without its trailing slash.
    AddSlash,
    NotFound,
}

pub struct Site {
    /// Canonical, so that symlinks pointing outside it can be detected.
    root: PathBuf,
    /// Keyed by URL path below the root, like `/css/site.css`.
    memory: HashMap<String, Arc<MemFile>>,
    fallback: Option<String>,
    cache_control: String,
}

impl Site {
    /// Takes the startup snapshot. `used` accumulates across routes so the
    /// total can be capped: running out of memory later, under load, is a much
    /// worse way to find out that a directory was bigger than expected.
    pub fn load(
        route: &str,
        config: &StaticSite,
        max_total: u64,
        used: &mut u64,
    ) -> Result<Site, BoxError> {
        let root = std::fs::canonicalize(&config.root)
            .map_err(|e| format!("route {route}: {}: {e}", config.root.display()))?;
        if !root.is_dir() {
            return Err(format!("route {route}: {} is not a directory", root.display()).into());
        }

        let mut memory = HashMap::new();
        let before = *used;
        if config.memory_max_file > 0 {
            snapshot(
                &root,
                "",
                config.memory_max_file,
                max_total,
                used,
                &mut memory,
            )
            .map_err(|e| format!("route {route}: {e}"))?;
        }
        info!(
            "route {route}: {} files ({} KiB) held in memory from {}",
            memory.len(),
            (*used - before).div_ceil(1024),
            root.display()
        );

        Ok(Site {
            root,
            memory,
            fallback: config.fallback.clone(),
            cache_control: config.cache_control.clone(),
        })
    }

    async fn find(&self, key: &str) -> Option<Found> {
        if let Some(file) = self.memory.get(key) {
            return Some(Found::Memory(file.clone()));
        }
        let path = tokio::fs::canonicalize(self.root.join(&key[1..]))
            .await
            .ok()?;
        // A symlink may point anywhere; only follow it inside the root.
        if !path.starts_with(&self.root) {
            return None;
        }
        let file = tokio::fs::File::open(&path).await.ok()?;
        let metadata = file.metadata().await.ok()?;
        metadata
            .is_file()
            .then(|| Found::Disk(file, Meta::new(&path, &metadata)))
    }

    async fn lookup(&self, segments: &[String], trailing_slash: bool) -> Lookup {
        let key = format!("/{}", segments.join("/"));
        let index = format!("{}/{INDEX}", key.trim_end_matches('/'));
        // Only possible below a path prefix: `/docs` for a route at `/docs`.
        // Serving the index there would break its relative links.
        if segments.is_empty() && !trailing_slash {
            return Lookup::AddSlash;
        }
        if trailing_slash {
            return match self.find(&index).await {
                Some(found) => Lookup::File(found),
                None => Lookup::NotFound,
            };
        }
        match self.find(&key).await {
            Some(found) => Lookup::File(found),
            None if self.find(&index).await.is_some() => Lookup::AddSlash,
            None => Lookup::NotFound,
        }
    }
}

fn snapshot(
    dir: &Path,
    url: &str,
    max_file: u64,
    max_total: u64,
    used: &mut u64,
    memory: &mut HashMap<String, Arc<MemFile>>,
) -> Result<(), BoxError> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !servable_segment(&name) {
            continue;
        }
        let key = format!("{url}/{name}");
        // `file_type` does not follow symlinks, which are left out of the
        // snapshot on purpose: no loops to guard against here, and they are
        // still served from disk, where the escape check happens.
        let kind = entry.file_type()?;
        if kind.is_dir() {
            snapshot(&entry.path(), &key, max_file, max_total, used, memory)?;
        } else if kind.is_file() {
            let metadata = entry.metadata()?;
            if metadata.len() > max_file {
                continue;
            }
            let path = entry.path();
            let body = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            *used += body.len() as u64;
            if *used > max_total {
                return Err(format!(
                    "files to hold in memory exceed PINGENV_MEMORY_MAX_TOTAL ({max_total} bytes); \
                     raise it, or lower PINGENV_ROUTE_<NAME>_MEMORY_MAX_FILE (0 serves from disk)"
                )
                .into());
            }
            // The length is taken from what was read, not from the earlier
            // stat, so Content-Length always matches the body.
            let meta = Meta {
                len: body.len() as u64,
                ..Meta::new(&path, &metadata)
            };
            memory.insert(
                key,
                Arc::new(MemFile {
                    meta,
                    body: Bytes::from(body),
                }),
            );
        }
    }
    Ok(())
}

fn servable_segment(segment: &str) -> bool {
    segment == ".well-known" || !segment.starts_with('.')
}

/// Turns the request path below the route prefix into validated segments.
/// `None` means the path must not be served at all.
fn clean_path(raw: &str) -> Option<(Vec<String>, bool)> {
    let decoded = percent_decode(raw)?;
    if decoded.contains(['\0', '\\']) {
        return None;
    }
    let mut segments = Vec::new();
    for segment in decoded.split('/').filter(|s| !s.is_empty()) {
        // Checked after decoding, so `%2e%2e` and `%2F..` get no further than
        // a literal `..` does.
        if segment == "." || segment == ".." || !servable_segment(segment) {
            return None;
        }
        segments.push(segment.to_string());
    }
    Some((segments, decoded.ends_with('/')))
}

fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[derive(Debug, PartialEq, Eq)]
enum ByteRange {
    Full,
    /// Inclusive on both ends.
    Partial(u64, u64),
    Unsatisfiable,
}

/// Single ranges only. Anything else (several ranges, other units, garbage) is
/// answered with the whole file, which the specification allows.
fn parse_range(header: &str, len: u64) -> ByteRange {
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return ByteRange::Full;
    };
    let Some((start, end)) = spec.split_once('-') else {
        return ByteRange::Full;
    };
    let (start, end) = (start.trim(), end.trim());
    if spec.contains(',') {
        return ByteRange::Full;
    }
    if start.is_empty() {
        // Suffix form: the last N bytes.
        return match end.parse::<u64>() {
            Ok(0) => ByteRange::Unsatisfiable,
            Ok(n) if len > 0 => ByteRange::Partial(len.saturating_sub(n), len - 1),
            Ok(_) => ByteRange::Unsatisfiable,
            Err(_) => ByteRange::Full,
        };
    }
    let Ok(start) = start.parse::<u64>() else {
        return ByteRange::Full;
    };
    let end = if end.is_empty() {
        len.saturating_sub(1)
    } else {
        match end.parse::<u64>() {
            Ok(end) => end.min(len.saturating_sub(1)),
            Err(_) => return ByteRange::Full,
        }
    };
    if start >= len {
        ByteRange::Unsatisfiable
    } else if start > end {
        ByteRange::Full
    } else {
        ByteRange::Partial(start, end)
    }
}

fn etag_matches(header: &str, etag: &str) -> bool {
    header.trim() == "*"
        || header
            .split(',')
            .map(|candidate| candidate.trim().trim_start_matches("W/"))
            .any(|candidate| candidate == etag)
}

fn content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref().unwrap_or("") {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        _ => "application/octet-stream",
    }
}

fn header(session: &Session, name: http::header::HeaderName) -> Option<String> {
    session
        .req_header()
        .headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

/// Answers the request from `site`. Returns where the response came from, for
/// the access log.
pub async fn serve(site: &Site, session: &mut Session, prefix: &str) -> Result<&'static str> {
    let req = session.req_header();
    let head = req.method == http::Method::HEAD;
    if !head && req.method != http::Method::GET {
        respond(
            session,
            405,
            &[("Allow", "GET, HEAD".into())],
            "method not allowed\n",
        )
        .await?;
        return Ok("-");
    }

    let path = req.uri.path().to_string();
    let query = req.uri.query().map(str::to_string);
    let below_prefix = if prefix == "/" {
        path.as_str()
    } else {
        &path[prefix.len()..]
    };
    let Some((segments, trailing_slash)) = clean_path(below_prefix) else {
        respond(session, 404, &[], "not found\n").await?;
        return Ok("-");
    };

    let found = match site.lookup(&segments, trailing_slash).await {
        Lookup::File(found) => Some(found),
        Lookup::AddSlash => {
            let location = match query {
                Some(q) => format!("{path}/?{q}"),
                None => format!("{path}/"),
            };
            respond(session, 308, &[("Location", location)], "").await?;
            return Ok("-");
        }
        // Only for paths that look like application routes: answering a
        // missing `/app.js` with HTML and a 200 hides the real problem.
        Lookup::NotFound => match &site.fallback {
            Some(fallback) if segments.last().is_none_or(|s| !s.contains('.')) => {
                site.find(fallback).await
            }
            _ => None,
        },
    };
    let Some(found) = found else {
        respond(session, 404, &[], "not found\n").await?;
        return Ok("-");
    };

    let meta = found.meta();
    let etag = meta.etag();
    let source = match found {
        Found::Memory(_) => "memory",
        Found::Disk(..) => "disk",
    };

    let common = |resp: &mut ResponseHeader| -> Result<()> {
        resp.insert_header("ETag", &etag)?;
        resp.insert_header("Last-Modified", meta.last_modified())?;
        resp.insert_header("Cache-Control", &site.cache_control)?;
        Ok(())
    };

    let not_modified = match header(session, http::header::IF_NONE_MATCH) {
        Some(candidates) => etag_matches(&candidates, &etag),
        None => header(session, http::header::IF_MODIFIED_SINCE)
            .and_then(|date| httpdate::parse_http_date(&date).ok())
            .and_then(|date| date.duration_since(SystemTime::UNIX_EPOCH).ok())
            .is_some_and(|since| meta.modified <= since.as_secs()),
    };
    if not_modified {
        let mut resp = ResponseHeader::build(304, Some(3))?;
        common(&mut resp)?;
        session.write_response_header(Box::new(resp), true).await?;
        return Ok(source);
    }

    let range = match header(session, http::header::RANGE) {
        // A range against a different version of the file gets all of it.
        Some(range) if header(session, http::header::IF_RANGE).is_none_or(|v| v.trim() == etag) => {
            parse_range(&range, meta.len)
        }
        _ => ByteRange::Full,
    };
    let (status, start, end) = match range {
        ByteRange::Full => (200, 0, meta.len),
        ByteRange::Partial(start, last) => (206, start, last + 1),
        ByteRange::Unsatisfiable => {
            let total = format!("bytes */{}", meta.len);
            respond(
                session,
                416,
                &[("Content-Range", total)],
                "range not satisfiable\n",
            )
            .await?;
            return Ok(source);
        }
    };

    let mut resp = ResponseHeader::build(status, Some(8))?;
    common(&mut resp)?;
    resp.insert_header("Content-Type", meta.content_type)?;
    resp.insert_header("Content-Length", (end - start).to_string())?;
    resp.insert_header("Accept-Ranges", "bytes")?;
    resp.insert_header("X-Content-Type-Options", "nosniff")?;
    if status == 206 {
        let content_range = format!("bytes {start}-{}/{}", end - 1, meta.len);
        resp.insert_header("Content-Range", content_range)?;
    }
    let no_body = head || start == end;
    session
        .write_response_header(Box::new(resp), no_body)
        .await?;
    if no_body {
        return Ok(source);
    }

    match found {
        Found::Memory(file) => {
            let body = file.body.slice(start as usize..end as usize);
            session.write_response_body(Some(body), true).await?;
        }
        Found::Disk(mut file, _) => {
            if start > 0 {
                file.seek(SeekFrom::Start(start))
                    .await
                    .or_err(ReadError, "seeking in file")?;
            }
            let mut remaining = end - start;
            while remaining > 0 {
                let want = remaining.min(CHUNK as u64) as usize;
                let mut chunk = BytesMut::zeroed(want);
                // Fails if the file shrank underneath us. The headers already
                // promised a length, so dropping the connection is the only
                // honest thing left to do.
                file.read_exact(&mut chunk)
                    .await
                    .or_err(ReadError, "reading file")?;
                remaining -= chunk.len() as u64;
                session
                    .write_response_body(Some(chunk.freeze()), remaining == 0)
                    .await?;
            }
        }
    }
    Ok(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(raw: &str) -> Option<(Vec<&'static str>, bool)> {
        clean_path(raw).map(|(segments, slash)| {
            let leaked: Vec<&'static str> = segments
                .into_iter()
                .map(|s| &*Box::leak(s.into_boxed_str()))
                .collect();
            (leaked, slash)
        })
    }

    #[test]
    fn paths_are_normalised() {
        assert_eq!(clean(""), Some((vec![], false)));
        assert_eq!(clean("/"), Some((vec![], true)));
        assert_eq!(clean("/a/b.css"), Some((vec!["a", "b.css"], false)));
        assert_eq!(clean("//a///b/"), Some((vec!["a", "b"], true)));
        assert_eq!(clean("/a%20b/%C3%A9"), Some((vec!["a b", "é"], false)));
        assert_eq!(
            clean("/.well-known/security.txt"),
            Some((vec![".well-known", "security.txt"], false))
        );
    }

    #[test]
    fn traversal_and_dotfiles_are_refused() {
        for bad in [
            "/..",
            "/../etc/passwd",
            "/a/../../etc/passwd",
            "/%2e%2e/etc/passwd",
            "/a/%2E%2E/b",
            "/a%2f..%2f..%2fetc/passwd",
            "/./a",
            "/.env",
            "/.git/config",
            "/a/.htpasswd",
            "/a\\..\\b",
            "/a%00.html",
            "/%zz",
            "/%2",
            "/%ff",
        ] {
            assert_eq!(clean(bad), None, "{bad}");
        }
    }

    #[test]
    fn ranges() {
        use ByteRange::*;
        assert_eq!(parse_range("bytes=0-99", 1000), Partial(0, 99));
        assert_eq!(parse_range("bytes=500-", 1000), Partial(500, 999));
        assert_eq!(parse_range("bytes=-100", 1000), Partial(900, 999));
        assert_eq!(parse_range("bytes=-5000", 1000), Partial(0, 999));
        assert_eq!(parse_range("bytes=990-2000", 1000), Partial(990, 999));
        assert_eq!(parse_range("bytes=0-0", 1000), Partial(0, 0));
        assert_eq!(parse_range("bytes=1000-", 1000), Unsatisfiable);
        assert_eq!(parse_range("bytes=-0", 1000), Unsatisfiable);
        assert_eq!(parse_range("bytes=0-", 0), Unsatisfiable);
        assert_eq!(parse_range("bytes=5-2", 1000), Full);
        assert_eq!(parse_range("bytes=0-1,5-6", 1000), Full);
        assert_eq!(parse_range("items=0-1", 1000), Full);
        assert_eq!(parse_range("bytes=a-b", 1000), Full);
    }

    #[test]
    fn etags() {
        let etag = "\"5f-10\"";
        assert!(etag_matches("\"5f-10\"", etag));
        assert!(etag_matches("W/\"5f-10\"", etag));
        assert!(etag_matches("\"other\", \"5f-10\"", etag));
        assert!(etag_matches(" * ", etag));
        assert!(!etag_matches("\"5f-11\"", etag));
    }

    #[test]
    fn content_types() {
        assert_eq!(
            content_type(Path::new("/x/INDEX.HTML")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("app.min.js")),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            content_type(Path::new("LICENSE")),
            "application/octet-stream"
        );
    }

    #[test]
    fn snapshot_respects_limits_and_skips_hidden_and_linked_entries() {
        let dir = std::env::temp_dir().join(format!("pingenv-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("css")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("index.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(dir.join("css/site.css"), "body{}").unwrap();
        std::fs::write(dir.join("big.bin"), vec![0u8; 5000]).unwrap();
        std::fs::write(dir.join(".env"), "SECRET=1").unwrap();
        std::fs::write(dir.join(".git/config"), "x").unwrap();
        std::os::unix::fs::symlink(dir.join("index.html"), dir.join("link.html")).unwrap();

        let config = StaticSite {
            root: dir.clone(),
            memory_max_file: 1000,
            fallback: None,
            cache_control: "no-cache".into(),
        };
        let mut used = 0;
        let site = Site::load("t", &config, 1 << 20, &mut used).unwrap();
        let mut keys: Vec<_> = site.memory.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["/css/site.css", "/index.html"]);
        assert_eq!(used, 17);

        let mut used = 0;
        let too_small = Site::load("t", &config, 10, &mut used);
        assert!(too_small.is_err_and(|e| e.to_string().contains("PINGENV_MEMORY_MAX_TOTAL")));

        let mut used = 0;
        let on_disk = StaticSite {
            memory_max_file: 0,
            ..config
        };
        let site = Site::load("t", &on_disk, 1 << 20, &mut used).unwrap();
        assert!(site.memory.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
