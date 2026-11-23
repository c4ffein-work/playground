//! git over http(s), "smart" protocol, on the workspace's own HTTP stack: `ureq` and
//! the `rustls` + `ring` agents of `pinned-http`. gitoxide speaks the protocol and
//! leaves the HTTP requests to whoever implements its `Http` trait: this does.

use std::io::{BufRead, BufReader, Cursor, Read, Write};
use std::sync::{Arc, Mutex};

use base64::Engine;
use gix_transport::client::blocking_io::http::{GetResponse, Http, PostBodyDataKind, PostResponse};
use pinned_http::{PinnedAgent, Trust};

use crate::{PassError, TlsTrust};

/// A git remote over http or https.
pub(crate) struct HttpRemote {
    pub(crate) url: String,
    agent: ureq::Agent,
    /// Set over https with a pinned certificate: tells a pin mismatch from another failure.
    pinned: Option<PinnedAgent>,
    authorization: Option<String>,
    /// Why the last request failed, in our own terms: gitoxide's error only has the text of it.
    failure: Mutex<Option<PassError>>,
}

fn invalid(message: &str) -> PassError {
    PassError::InvalidConfig {
        message: message.to_string(),
    }
}

impl HttpRemote {
    /// Validates everything. Connects to nothing.
    pub(crate) fn new(
        url: &str,
        username: Option<&str>,
        password: Option<&str>,
        trust: Option<&TlsTrust>,
    ) -> Result<Arc<Self>, PassError> {
        let url = url.trim().trim_end_matches('/').to_string();
        let https = url.to_ascii_lowercase().starts_with("https://");
        let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or("");
        let (authority, path) = after_scheme.split_once('/').unwrap_or((after_scheme, ""));
        if authority.is_empty() || authority.contains('@') || path.is_empty() || url.contains(['?', '#']) {
            return Err(invalid(
                "the repository URL is http(s)://host[:port]/path, with no user name in it",
            ));
        }
        if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(invalid("the repository URL holds a space or a control character"));
        }

        let password = password.unwrap_or("");
        let authorization = if password.is_empty() {
            None
        } else if !https {
            // The store itself is encrypted, the token is not: it would cross the network in clear.
            return Err(invalid("a password or token is only sent over https://"));
        } else {
            let username = username.map(str::trim).filter(|u| !u.is_empty()).unwrap_or("git");
            if username.contains(':') || username.chars().chain(password.chars()).any(char::is_control) {
                return Err(invalid(
                    "the user name or the token holds a character that cannot be sent",
                ));
            }
            let pair = zeroize::Zeroizing::new(format!("{username}:{password}"));
            Some(format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(pair.as_bytes())
            ))
        };

        let (agent, pinned) = match trust {
            Some(TlsTrust::PublicCa) if https => (pinned_http::web_agent(0).map_err(config_error)?, None),
            Some(TlsTrust::PublicCa) => return Err(invalid("a certificate check is set but the URL is http://")),
            trust => {
                let trust = trust.and_then(|trust| match trust.clone() {
                    TlsTrust::PinnedWithCa { sha256, extra_ca_pem } => Some(Trust::PinnedWithCa {
                        pins: vec![sha256],
                        extra_ca_pem,
                    }),
                    TlsTrust::PinnedOnly { sha256 } => Some(Trust::PinnedOnly { pins: vec![sha256] }),
                    TlsTrust::PublicCa => None,
                });
                let pinned = pinned_http::agent(&url, trust.as_ref()).map_err(config_error)?;
                (pinned.agent.clone(), Some(pinned))
            }
        };
        Ok(Arc::new(Self {
            url,
            agent,
            pinned,
            authorization,
            failure: Mutex::new(None),
        }))
    }

    /// The reason of the last failed request, once.
    pub(crate) fn take_failure(&self) -> Option<PassError> {
        self.failure.lock().unwrap().take()
    }

    fn fail(&self, error: PassError) -> std::io::Error {
        let kind = match error {
            PassError::Unauthorized => std::io::ErrorKind::PermissionDenied,
            _ => std::io::ErrorKind::Other,
        };
        let text = error.to_string();
        *self.failure.lock().unwrap() = Some(error);
        std::io::Error::new(kind, text)
    }

    /// Sends one request. Anything but a 200 is an error: redirects are not followed,
    /// they could take the credentials to another host.
    fn send(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<&[u8]>,
    ) -> std::io::Result<ureq::Response> {
        let mut request = self.agent.request(method, url);
        for (name, value) in headers {
            // gitoxide adds its own Authorization when the URL holds credentials: ours never does.
            if !name.eq_ignore_ascii_case("authorization") {
                request = request.set(name, value);
            }
        }
        if let Some(authorization) = &self.authorization {
            request = request.set("Authorization", authorization);
        }
        let result = match body {
            Some(body) => request.send_bytes(body),
            None => request.call(),
        };
        let status = match result {
            Ok(response) if response.status() == 200 => return Ok(response),
            Ok(response) => response.status(),
            Err(ureq::Error::Status(status, _)) => status,
            Err(ureq::Error::Transport(transport)) => {
                let served = self.pinned.as_ref().and_then(PinnedAgent::pin_mismatch);
                return Err(self.fail(match (served, &self.pinned) {
                    (Some(served), Some(pinned)) => PassError::PinMismatch {
                        pinned: pinned.pins.join(", "),
                        served,
                    },
                    _ => PassError::Network {
                        message: transport.to_string(),
                    },
                }));
            }
        };
        Err(self.fail(match status {
            401 | 403 => PassError::Unauthorized,
            404 => PassError::Network {
                message: "no git repository at this URL (404)".into(),
            },
            300..=399 => PassError::Network {
                message: format!("the server redirects ({status}): use the repository's final URL"),
            },
            _ => PassError::Network {
                message: format!("the server answered {status}"),
            },
        }))
    }

    /// `GET <url>/info/refs?service=git-receive-pack`: what the remote has, for a push.
    pub(crate) fn receive_pack_advertisement(&self) -> Result<Vec<u8>, PassError> {
        let url = format!("{}/info/refs?service=git-receive-pack", self.url);
        let response = self.send("GET", &url, &[], None).map_err(|e| self.error_of(e))?;
        let smart = response.content_type() == "application/x-git-receive-pack-advertisement";
        let mut body = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut body)
            .map_err(|e| self.error_of(e))?;
        if !smart {
            return Err(PassError::Network {
                message: "the server does not accept git pushes at this URL".into(),
            });
        }
        Ok(body)
    }

    /// `POST <url>/git-receive-pack`: the ref updates and the pack. Returns the server's report.
    pub(crate) fn receive_pack(&self, request: &[u8]) -> Result<Vec<u8>, PassError> {
        let url = format!("{}/git-receive-pack", self.url);
        let headers = [
            (
                "Content-Type".to_string(),
                "application/x-git-receive-pack-request".to_string(),
            ),
            (
                "Accept".to_string(),
                "application/x-git-receive-pack-result".to_string(),
            ),
        ];
        let response = self
            .send("POST", &url, &headers, Some(request))
            .map_err(|e| self.error_of(e))?;
        let mut body = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut body)
            .map_err(|e| self.error_of(e))?;
        Ok(body)
    }

    fn error_of(&self, e: std::io::Error) -> PassError {
        self.take_failure()
            .unwrap_or_else(|| PassError::Network { message: e.to_string() })
    }
}

fn config_error(e: pinned_http::Error) -> PassError {
    invalid(&e.to_string())
}

fn split_header(line: &str) -> Option<(String, String)> {
    let (name, value) = line.split_once(':')?;
    Some((name.trim().to_string(), value.trim().to_string()))
}

/// The response headers as lines, which is how gitoxide wants to read them.
fn header_lines(response: &ureq::Response) -> Cursor<Vec<u8>> {
    let mut lines = Vec::new();
    for name in response.headers_names() {
        if let Some(value) = response.header(&name) {
            lines.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
    }
    Cursor::new(lines)
}

type Body = BufReader<Box<dyn Read + Send + Sync>>;

/// One POST. gitoxide first writes the request body, then reads the response headers,
/// then the response body: the request leaves when the first of the two is read.
struct Exchange {
    remote: Arc<HttpRemote>,
    url: String,
    headers: Vec<(String, String)>,
    request: Vec<u8>,
    sent: bool,
    response_headers: Option<Cursor<Vec<u8>>>,
    response_body: Option<Body>,
}

impl Exchange {
    fn send(&mut self) -> std::io::Result<()> {
        if self.sent {
            return Ok(());
        }
        self.sent = true;
        let request = std::mem::take(&mut self.request);
        let response = self.remote.send("POST", &self.url, &self.headers, Some(&request))?;
        self.response_headers = Some(header_lines(&response));
        self.response_body = Some(BufReader::new(response.into_reader()));
        Ok(())
    }
}

pub(crate) struct PostBody(Arc<Mutex<Exchange>>);

impl Write for PostBody {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().request.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Response headers, or a response body, of a request that may not have been sent yet.
pub(crate) struct Lazy<T> {
    exchange: Option<Arc<Mutex<Exchange>>>,
    take: fn(&mut Exchange) -> Option<T>,
    inner: Option<T>,
}

impl<T> Lazy<T> {
    fn ready(inner: T) -> Self {
        Self {
            exchange: None,
            take: |_| None,
            inner: Some(inner),
        }
    }

    fn get(&mut self) -> std::io::Result<&mut T> {
        if self.inner.is_none() {
            let exchange = self.exchange.as_ref().expect("either ready or lazy");
            let mut exchange = exchange.lock().unwrap();
            exchange.send()?;
            self.inner = (self.take)(&mut exchange);
        }
        self.inner
            .as_mut()
            .ok_or_else(|| std::io::Error::other("the request has failed"))
    }
}

impl<T: Read> Read for Lazy<T> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.get()?.read(buf)
    }
}

impl<T: BufRead> BufRead for Lazy<T> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        self.get()?.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        if let Some(inner) = self.inner.as_mut() {
            inner.consume(amount);
        }
    }
}

/// gitoxide's HTTP backend, for a fetch.
pub(crate) struct GitHttp(pub(crate) Arc<HttpRemote>);

fn collect_headers(headers: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<(String, String)> {
    headers
        .into_iter()
        .filter_map(|line| split_header(line.as_ref()))
        .collect()
}

impl Http for GitHttp {
    type Headers = Lazy<Cursor<Vec<u8>>>;
    type ResponseBody = Lazy<Body>;
    type PostBody = PostBody;

    fn get(
        &mut self,
        url: &str,
        _base_url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<GetResponse<Self::Headers, Self::ResponseBody>, gix::Error> {
        let response = self
            .0
            .send("GET", url, &collect_headers(headers), None)
            .map_err(gix::Error::from_error)?;
        Ok(GetResponse {
            headers: Lazy::ready(header_lines(&response)),
            body: Lazy::ready(BufReader::new(response.into_reader())),
        })
    }

    fn post(
        &mut self,
        url: &str,
        _base_url: &str,
        headers: impl IntoIterator<Item = impl AsRef<str>>,
        _body: PostBodyDataKind,
    ) -> Result<PostResponse<Self::Headers, Self::ResponseBody, Self::PostBody>, gix::Error> {
        let exchange = Arc::new(Mutex::new(Exchange {
            remote: self.0.clone(),
            url: url.to_string(),
            headers: collect_headers(headers),
            request: Vec::new(),
            sent: false,
            response_headers: None,
            response_body: None,
        }));
        Ok(PostResponse {
            post_body: PostBody(exchange.clone()),
            headers: Lazy {
                exchange: Some(exchange.clone()),
                take: |exchange| exchange.response_headers.take(),
                inner: None,
            },
            body: Lazy {
                exchange: Some(exchange),
                take: |exchange| exchange.response_body.take(),
                inner: None,
            },
        })
    }

    fn configure(&mut self, _config: &dyn std::any::Any) -> Result<(), gix::Error> {
        Ok(())
    }
}
