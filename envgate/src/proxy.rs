//! The request path: answer ACME challenges, pick a route by host and path,
//! redirect plain HTTP to HTTPS, and forward everything else upstream.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use log::{info, warn};
use pingora::http::{RequestHeader, ResponseHeader};
use pingora::prelude::*;

use crate::acme::{CHALLENGE_PREFIX, ChallengeStore};
use crate::config::{Config, Route, Upstream};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Upstream names are re-resolved this often, so a container that comes back
/// with a new address is picked up without restarting the proxy.
const DNS_TTL: Duration = Duration::from_secs(10);
const MAX_CONNECT_RETRIES: u32 = 2;

pub struct Router {
    routes: Vec<Route>,
}

impl Router {
    pub fn new(routes: Vec<Route>) -> Self {
        Self { routes }
    }

    /// Exact hosts win over wildcards; within a host the longest matching path
    /// prefix wins.
    pub fn find(&self, host: &str, path: &str) -> Option<usize> {
        let wildcard = host
            .split_once('.')
            .map(|(_, parent)| format!("*.{parent}"));
        let best = |want: &str| {
            self.routes
                .iter()
                .enumerate()
                .filter(|(_, r)| r.hosts.iter().any(|h| h == want) && path_matches(&r.path, path))
                .max_by_key(|(_, r)| r.path.len())
                .map(|(i, _)| i)
        };
        best(host).or_else(|| wildcard.as_deref().and_then(best))
    }
}

/// `/api` matches `/api` and `/api/x` but not `/apix`.
fn path_matches(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn strip_prefix(prefix: &str, path_and_query: &str) -> String {
    if prefix == "/" {
        return path_and_query.to_string();
    }
    let rest = &path_and_query[prefix.len()..];
    if rest.starts_with('/') {
        rest.to_string()
    } else {
        format!("/{rest}")
    }
}

/// Host the client asked for, lowercased and without a port.
fn request_host(req: &RequestHeader) -> Option<String> {
    let raw = req
        .uri
        .host()
        .or_else(|| req.headers.get(http::header::HOST)?.to_str().ok())?;
    let host = match raw.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => raw.split(':').next()?,
    };
    (!host.is_empty()).then(|| host.trim_end_matches('.').to_ascii_lowercase())
}

#[derive(Default)]
struct Resolver {
    cache: Mutex<HashMap<String, (Instant, Vec<SocketAddr>)>>,
}

impl Resolver {
    async fn resolve(&self, upstream: &Upstream) -> Result<Vec<SocketAddr>> {
        let authority = upstream.authority();
        if let Ok(addr) = authority.parse::<SocketAddr>() {
            return Ok(vec![addr]);
        }
        let cached = self.cache.lock().unwrap().get(&authority).cloned();
        if let Some((at, addrs)) = &cached
            && at.elapsed() < DNS_TTL
        {
            return Ok(addrs.clone());
        }
        let looked_up = tokio::net::lookup_host(&authority)
            .await
            .map(|found| found.collect::<Vec<SocketAddr>>());
        match looked_up {
            Ok(addrs) => {
                if addrs.is_empty() {
                    return Error::e_explain(ConnectNoRoute, format!("{authority}: no addresses"));
                }
                self.cache
                    .lock()
                    .unwrap()
                    .insert(authority, (Instant::now(), addrs.clone()));
                Ok(addrs)
            }
            // A resolver hiccup should not take down an upstream that was
            // reachable a moment ago.
            Err(e) => match cached {
                Some((_, addrs)) => {
                    warn!("{authority}: lookup failed ({e}), using last known address");
                    Ok(addrs)
                }
                None => Error::e_explain(ConnectNoRoute, format!("{authority}: {e}")),
            },
        }
    }
}

pub struct Gateway {
    router: Router,
    counters: Vec<AtomicUsize>,
    resolver: Resolver,
    challenges: Arc<ChallengeStore>,
    https_redirect: bool,
    https_redirect_port: u16,
}

impl Gateway {
    pub fn new(config: &Config, challenges: Arc<ChallengeStore>) -> Self {
        Self {
            counters: config.routes.iter().map(|_| AtomicUsize::new(0)).collect(),
            router: Router::new(config.routes.clone()),
            resolver: Resolver::default(),
            challenges,
            https_redirect: config.https_redirect,
            https_redirect_port: config.https_redirect_port,
        }
    }
}

#[derive(Default)]
pub struct RequestCtx {
    route: Option<usize>,
    host: String,
    upstream: Option<String>,
    connect_failures: u32,
    started: Option<Instant>,
}

fn is_tls(session: &Session) -> bool {
    session.digest().is_some_and(|d| d.ssl_digest.is_some())
}

async fn respond(
    session: &mut Session,
    status: u16,
    headers: &[(&'static str, String)],
    body: &'static str,
) -> Result<()> {
    let mut resp = ResponseHeader::build(status, Some(headers.len() + 2))?;
    resp.insert_header("Content-Type", "text/plain; charset=utf-8")?;
    resp.insert_header("Content-Length", body.len().to_string())?;
    for (name, value) in headers {
        resp.insert_header(*name, value)?;
    }
    session
        .write_response_header(Box::new(resp), body.is_empty())
        .await?;
    if !body.is_empty() {
        session
            .write_response_body(Some(Bytes::from_static(body.as_bytes())), true)
            .await?;
    }
    Ok(())
}

#[async_trait]
impl ProxyHttp for Gateway {
    type CTX = RequestCtx;

    fn new_ctx(&self) -> RequestCtx {
        RequestCtx::default()
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut RequestCtx) -> Result<bool> {
        ctx.started = Some(Instant::now());
        let tls = is_tls(session);
        let req = session.req_header();
        let path = req.uri.path().to_string();

        // Before routing and before the redirect: the CA validates over plain
        // HTTP, and it does so before the host has any certificate.
        if !tls && let Some(token) = path.strip_prefix(CHALLENGE_PREFIX) {
            return match self.challenges.get(token) {
                Some(key_authorization) => {
                    let mut resp = ResponseHeader::build(200, Some(2))?;
                    resp.insert_header("Content-Type", "application/octet-stream")?;
                    resp.insert_header("Content-Length", key_authorization.len().to_string())?;
                    session.write_response_header(Box::new(resp), false).await?;
                    session
                        .write_response_body(Some(Bytes::from(key_authorization)), true)
                        .await?;
                    Ok(true)
                }
                None => respond(session, 404, &[], "unknown challenge\n")
                    .await
                    .map(|_| true),
            };
        }

        let Some(host) = request_host(req) else {
            respond(session, 400, &[], "missing host\n").await?;
            return Ok(true);
        };
        let Some(route) = self.router.find(&host, &path) else {
            ctx.host = host;
            respond(session, 404, &[], "no route for this host\n").await?;
            return Ok(true);
        };
        ctx.route = Some(route);

        if !tls && self.https_redirect {
            let target = req.uri.path_and_query().map_or("/", |pq| pq.as_str());
            let location = match self.https_redirect_port {
                443 => format!("https://{host}{target}"),
                port => format!("https://{host}:{port}{target}"),
            };
            ctx.host = host;
            respond(session, 308, &[("Location", location)], "").await?;
            return Ok(true);
        }
        ctx.host = host;
        Ok(false)
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut RequestCtx,
    ) -> Result<Box<HttpPeer>> {
        let index = ctx.route.expect("request_filter sets the route");
        let route = &self.router.routes[index];
        // Round robin across upstreams; a retry lands on the next one.
        let turn = self.counters[index].fetch_add(1, Ordering::Relaxed);
        let upstream = &route.upstreams[turn % route.upstreams.len()];
        let addrs = self.resolver.resolve(upstream).await?;
        let addr = addrs[turn / route.upstreams.len() % addrs.len()];

        ctx.upstream = Some(upstream.authority());
        let mut peer = HttpPeer::new(addr, upstream.tls, upstream.host.clone());
        peer.options.connection_timeout = Some(CONNECT_TIMEOUT);
        Ok(Box::new(peer))
    }

    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut RequestCtx,
    ) -> Result<()> {
        let route = &self.router.routes[ctx.route.expect("request_filter sets the route")];

        // These are overwritten, not appended to: this proxy is the edge, so
        // whatever the client sent in them is not to be trusted.
        if let Some(addr) = session.client_addr().and_then(|a| a.as_inet()) {
            upstream_request.insert_header("X-Forwarded-For", addr.ip().to_string())?;
        }
        let proto = if is_tls(session) { "https" } else { "http" };
        upstream_request.insert_header("X-Forwarded-Proto", proto)?;
        upstream_request.insert_header("X-Forwarded-Host", &ctx.host)?;

        match &route.set_host {
            Some(host) => {
                upstream_request.insert_header("Host", host)?;
            }
            // HTTP/2 requests carry the host in :authority and have no Host
            // header, which an HTTP/1.1 upstream requires.
            None if upstream_request.headers.get(http::header::HOST).is_none() => {
                let authority = upstream_request
                    .uri
                    .authority()
                    .map_or_else(|| ctx.host.clone(), |a| a.to_string());
                upstream_request.insert_header("Host", authority)?;
            }
            None => {}
        }

        if route.strip_prefix && route.path != "/" {
            let original = upstream_request
                .uri
                .path_and_query()
                .map_or("/", |pq| pq.as_str());
            let rewritten = strip_prefix(&route.path, original);
            let uri = rewritten
                .parse::<http::Uri>()
                .or_err(InternalError, "rewritten path is not a valid URI")?;
            upstream_request.set_uri(uri);
        }
        Ok(())
    }

    fn fail_to_connect(
        &self,
        _session: &mut Session,
        _peer: &HttpPeer,
        ctx: &mut RequestCtx,
        mut e: Box<Error>,
    ) -> Box<Error> {
        // Nothing has been sent upstream yet, so trying again is always safe.
        if ctx.connect_failures < MAX_CONNECT_RETRIES {
            ctx.connect_failures += 1;
            e.set_retry(true);
        }
        e
    }

    async fn logging(&self, session: &mut Session, _e: Option<&Error>, ctx: &mut RequestCtx) {
        let req = session.req_header();
        let status = session.response_written().map_or(0, |r| r.status.as_u16());
        let client = session
            .client_addr()
            .and_then(|a| a.as_inet())
            .map_or_else(|| "-".to_string(), |a| a.ip().to_string());
        let route = ctx
            .route
            .map_or("-", |i| self.router.routes[i].name.as_str());
        // The query string is left out on purpose: it often carries tokens.
        info!(
            target: "access",
            "{client} {} {}{} {status} route={route} upstream={} {}ms",
            req.method,
            if ctx.host.is_empty() { "-" } else { &ctx.host },
            req.uri.path(),
            ctx.upstream.as_deref().unwrap_or("-"),
            ctx.started.map_or(0, |s| s.elapsed().as_millis()),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(name: &str, hosts: &[&str], path: &str) -> Route {
        Route {
            name: name.into(),
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            path: path.into(),
            strip_prefix: false,
            upstreams: vec![],
            set_host: None,
            cert_files: None,
        }
    }

    fn name(router: &Router, host: &str, path: &str) -> Option<String> {
        router
            .find(host, path)
            .map(|i| router.routes[i].name.clone())
    }

    #[test]
    fn routing_precedence() {
        let router = Router::new(vec![
            route("root", &["example.com"], "/"),
            route("api", &["example.com"], "/api"),
            route("deep", &["example.com"], "/api/v2"),
            route("wild", &["*.example.com"], "/"),
            route("exact", &["a.example.com"], "/only"),
        ]);
        assert_eq!(name(&router, "example.com", "/").as_deref(), Some("root"));
        assert_eq!(
            name(&router, "example.com", "/apix").as_deref(),
            Some("root")
        );
        assert_eq!(name(&router, "example.com", "/api").as_deref(), Some("api"));
        assert_eq!(
            name(&router, "example.com", "/api/users").as_deref(),
            Some("api")
        );
        assert_eq!(
            name(&router, "example.com", "/api/v2/x").as_deref(),
            Some("deep")
        );
        assert_eq!(
            name(&router, "b.example.com", "/x").as_deref(),
            Some("wild")
        );
        // An exact host that has no matching path falls back to the wildcard.
        assert_eq!(
            name(&router, "a.example.com", "/only/1").as_deref(),
            Some("exact")
        );
        assert_eq!(
            name(&router, "a.example.com", "/else").as_deref(),
            Some("wild")
        );
        assert_eq!(name(&router, "x.y.example.com", "/"), None);
        assert_eq!(name(&router, "other.test", "/"), None);
    }

    #[test]
    fn prefix_stripping_keeps_the_query() {
        assert_eq!(strip_prefix("/api", "/api"), "/");
        assert_eq!(strip_prefix("/api", "/api/"), "/");
        assert_eq!(strip_prefix("/api", "/api/users?id=1"), "/users?id=1");
        assert_eq!(strip_prefix("/api", "/api?id=1"), "/?id=1");
        assert_eq!(strip_prefix("/", "/x?y"), "/x?y");
    }

    #[test]
    fn host_is_normalised() {
        let mut req = RequestHeader::build("GET", b"/", None).unwrap();
        assert_eq!(request_host(&req), None);
        req.insert_header("Host", "Example.COM.:8443").unwrap();
        assert_eq!(request_host(&req).as_deref(), Some("example.com"));
        req.insert_header("Host", "[::1]:80").unwrap();
        assert_eq!(request_host(&req).as_deref(), Some("::1"));
    }
}
