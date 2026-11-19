//! Configuration, read only from environment variables.
//!
//! Parsing takes an iterator of pairs rather than reading the process
//! environment directly, so the whole thing is testable. Every problem found is
//! collected and reported at once: fixing a container's environment one error
//! per restart is miserable.

use std::collections::{BTreeMap, HashSet};
use std::net::SocketAddr;
use std::path::PathBuf;

pub const LETS_ENCRYPT_PRODUCTION: &str = "https://acme-v02.api.letsencrypt.org/directory";
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// How hosts without their own certificate files get a certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    /// No HTTPS listener at all.
    Off,
    /// Certificates are requested from an ACME CA (HTTP-01).
    Acme,
    /// A self-signed certificate is generated at startup. For local use.
    SelfSigned,
    /// Every route must name its own certificate and key files.
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub host: String,
    pub port: u16,
    pub tls: bool,
}

impl Upstream {
    pub fn authority(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertFiles {
    pub cert: PathBuf,
    pub key: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Route {
    /// The `<NAME>` part of `ROUTE_<NAME>_*`, lowercased. Used in logs.
    pub name: String,
    /// Lowercase host names; `*.example.com` matches one extra label.
    pub hosts: Vec<String>,
    /// Path prefix, always starting with `/` and without a trailing `/`
    /// (except for the root itself).
    pub path: String,
    pub strip_prefix: bool,
    pub upstreams: Vec<Upstream>,
    /// Overrides the `Host` header sent to the upstream.
    pub set_host: Option<String>,
    pub cert_files: Option<CertFiles>,
}

#[derive(Debug, Clone)]
pub struct AcmeConfig {
    pub directory_url: String,
    pub email: Option<String>,
    /// Extra root certificate to trust when talking to the ACME server.
    pub ca_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub http_addr: SocketAddr,
    pub https_addr: SocketAddr,
    pub tls: TlsMode,
    pub https_redirect: bool,
    /// Port used in redirect `Location` headers; differs from the listen port
    /// when the container port is published on another one.
    pub https_redirect_port: u16,
    pub data_dir: PathBuf,
    pub acme: AcmeConfig,
    pub routes: Vec<Route>,
    pub threads: usize,
    pub shutdown_grace_seconds: u64,
    pub log_level: log::LevelFilter,
}

const ROUTE_FIELDS: &[&str] = &[
    "TLS_CERT_FILE",
    "TLS_KEY_FILE",
    "STRIP_PREFIX",
    "SET_HOST",
    "UPSTREAM",
    "HOST",
    "PATH",
];

impl Config {
    pub fn from_env<I>(vars: I) -> Result<Config, Vec<String>>
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut errors = Vec::new();
        let mut globals: BTreeMap<String, String> = BTreeMap::new();
        let mut raw_routes: BTreeMap<String, BTreeMap<&'static str, String>> = BTreeMap::new();

        for (key, value) in vars {
            let value = value.trim().to_string();
            if let Some(rest) = key.strip_prefix("ROUTE_") {
                // Longest field name first, so `ROUTE_A_TLS_CERT_FILE` is not
                // read as a route called `A_TLS_CERT` with a `FILE` field.
                let field = ROUTE_FIELDS.iter().find_map(|f| {
                    let name = rest.strip_suffix(f)?.strip_suffix('_')?;
                    (!name.is_empty()).then_some((name, *f))
                });
                match field {
                    Some((name, field)) => {
                        raw_routes
                            .entry(name.to_string())
                            .or_default()
                            .insert(field, value);
                    }
                    None => errors.push(format!(
                        "{key}: unknown route setting (expected ROUTE_<NAME>_ followed by one of {})",
                        ROUTE_FIELDS.join(", ")
                    )),
                }
            } else {
                globals.insert(key, value);
            }
        }

        let get = |k: &str| globals.get(k).filter(|v| !v.is_empty()).cloned();

        let tls = match get("TLS_MODE")
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("off") => TlsMode::Off,
            Some("acme") => TlsMode::Acme,
            Some("selfsigned") | Some("self-signed") => TlsMode::SelfSigned,
            Some("manual") => TlsMode::Manual,
            Some(other) => {
                errors.push(format!(
                    "TLS_MODE: '{other}' is not one of off, acme, selfsigned, manual"
                ));
                TlsMode::Off
            }
        };

        let http_addr = parse_addr(&mut errors, "HTTP_ADDR", get("HTTP_ADDR"), "0.0.0.0:80");
        let https_addr = parse_addr(&mut errors, "HTTPS_ADDR", get("HTTPS_ADDR"), "0.0.0.0:443");
        let https_redirect = parse_bool(
            &mut errors,
            "HTTPS_REDIRECT",
            get("HTTPS_REDIRECT"),
            tls != TlsMode::Off,
        );
        if https_redirect && tls == TlsMode::Off {
            errors.push("HTTPS_REDIRECT: cannot redirect to HTTPS while TLS_MODE is off".into());
        }
        let https_redirect_port = parse_num(
            &mut errors,
            "HTTPS_REDIRECT_PORT",
            get("HTTPS_REDIRECT_PORT"),
            443u16,
        );

        let staging = parse_bool(&mut errors, "ACME_STAGING", get("ACME_STAGING"), false);
        let directory_url = match get("ACME_DIRECTORY_URL") {
            Some(url) => {
                if staging {
                    errors.push("ACME_STAGING and ACME_DIRECTORY_URL are both set; use one".into());
                }
                if !url.starts_with("https://") {
                    errors.push("ACME_DIRECTORY_URL: must start with https://".into());
                }
                url
            }
            None if staging => LETS_ENCRYPT_STAGING.to_string(),
            None => LETS_ENCRYPT_PRODUCTION.to_string(),
        };
        let email = get("ACME_EMAIL");
        if let Some(e) = &email
            && (!e.contains('@') || e.contains(char::is_whitespace))
        {
            errors.push(format!(
                "ACME_EMAIL: '{e}' does not look like an email address"
            ));
        }

        let default_threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let threads = parse_num(&mut errors, "THREADS", get("THREADS"), default_threads);
        if threads == 0 {
            errors.push("THREADS: must be at least 1".into());
        }
        let shutdown_grace_seconds = parse_num(
            &mut errors,
            "SHUTDOWN_GRACE_SECONDS",
            get("SHUTDOWN_GRACE_SECONDS"),
            5u64,
        );
        let log_level = match get("LOG_LEVEL") {
            None => log::LevelFilter::Info,
            Some(v) => v.parse().unwrap_or_else(|_| {
                errors.push(format!(
                    "LOG_LEVEL: '{v}' is not one of off, error, warn, info, debug, trace"
                ));
                log::LevelFilter::Info
            }),
        };

        let mut routes = Vec::new();
        for (name, fields) in &raw_routes {
            if let Some(route) = parse_route(&mut errors, name, fields) {
                routes.push(route);
            }
        }
        if raw_routes.is_empty() {
            errors.push(
                "no routes configured: set ROUTE_<NAME>_HOST and ROUTE_<NAME>_UPSTREAM".into(),
            );
        }

        let mut seen = HashSet::new();
        for route in &routes {
            for host in &route.hosts {
                if !seen.insert((host.clone(), route.path.clone())) {
                    errors.push(format!(
                        "ROUTE_{}_HOST: {host}{} is already served by another route",
                        route.name.to_uppercase(),
                        route.path
                    ));
                }
            }
            let var = |f: &str| format!("ROUTE_{}_{f}", route.name.to_uppercase());
            match (tls, &route.cert_files) {
                (TlsMode::Off, Some(_)) => errors.push(format!(
                    "{}: certificate files are set but TLS_MODE is off",
                    var("TLS_CERT_FILE")
                )),
                (TlsMode::Manual, None) => errors.push(format!(
                    "{} and {}: required when TLS_MODE is manual",
                    var("TLS_CERT_FILE"),
                    var("TLS_KEY_FILE")
                )),
                (TlsMode::Acme, None) => {
                    for host in route.hosts.iter().filter(|h| h.starts_with("*.")) {
                        errors.push(format!(
                            "{}: {host} is a wildcard, which HTTP-01 cannot validate; \
                             give this route certificate files instead",
                            var("HOST")
                        ));
                    }
                }
                _ => {}
            }
        }

        if !errors.is_empty() {
            errors.sort();
            errors.dedup();
            return Err(errors);
        }

        Ok(Config {
            http_addr,
            https_addr,
            tls,
            https_redirect,
            https_redirect_port,
            data_dir: PathBuf::from(get("DATA_DIR").unwrap_or_else(|| "/data".into())),
            acme: AcmeConfig {
                directory_url,
                email,
                ca_file: get("ACME_CA_FILE").map(PathBuf::from),
            },
            routes,
            threads,
            shutdown_grace_seconds,
            log_level,
        })
    }

    /// Hosts whose certificates are obtained from the ACME CA.
    pub fn acme_hosts(&self) -> Vec<String> {
        if self.tls != TlsMode::Acme {
            return Vec::new();
        }
        let mut hosts: Vec<String> = self
            .routes
            .iter()
            .filter(|r| r.cert_files.is_none())
            .flat_map(|r| r.hosts.iter().cloned())
            .collect();
        hosts.sort();
        hosts.dedup();
        hosts
    }
}

fn parse_route(
    errors: &mut Vec<String>,
    name: &str,
    fields: &BTreeMap<&'static str, String>,
) -> Option<Route> {
    let var = |f: &str| format!("ROUTE_{name}_{f}");
    let field = |f: &str| fields.get(f).filter(|v| !v.is_empty());
    let before = errors.len();

    if !name
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
    {
        errors.push(format!(
            "ROUTE_{name}_*: route names may only contain A-Z, 0-9 and _"
        ));
    }

    let mut hosts = Vec::new();
    match field("HOST") {
        None => errors.push(format!("{}: missing", var("HOST"))),
        Some(raw) => {
            for host in raw.split(',').map(str::trim).filter(|h| !h.is_empty()) {
                let host = host.to_ascii_lowercase();
                match validate_host(&host, true) {
                    Ok(()) => hosts.push(host),
                    Err(why) => errors.push(format!("{}: '{host}' {why}", var("HOST"))),
                }
            }
        }
    }

    let mut upstreams = Vec::new();
    match field("UPSTREAM") {
        None => errors.push(format!("{}: missing", var("UPSTREAM"))),
        Some(raw) => {
            for item in raw.split(',').map(str::trim).filter(|u| !u.is_empty()) {
                match parse_upstream(item) {
                    Ok(u) => upstreams.push(u),
                    Err(why) => errors.push(format!("{}: '{item}' {why}", var("UPSTREAM"))),
                }
            }
        }
    }

    let mut path = field("PATH").cloned().unwrap_or_else(|| "/".into());
    if !path.starts_with('/') {
        errors.push(format!("{}: '{path}' must start with /", var("PATH")));
    }
    while path.len() > 1 && path.ends_with('/') {
        path.pop();
    }

    let strip_prefix = parse_bool(
        errors,
        &var("STRIP_PREFIX"),
        field("STRIP_PREFIX").cloned(),
        false,
    );

    let set_host = field("SET_HOST").cloned();
    if let Some(h) = &set_host {
        let name_part = h.rsplit_once(':').map_or(h.as_str(), |(n, _)| n);
        if let Err(why) = validate_host(&name_part.to_ascii_lowercase(), false) {
            errors.push(format!("{}: '{h}' {why}", var("SET_HOST")));
        }
    }

    let cert_files = match (field("TLS_CERT_FILE"), field("TLS_KEY_FILE")) {
        (Some(cert), Some(key)) => Some(CertFiles {
            cert: PathBuf::from(cert),
            key: PathBuf::from(key),
        }),
        (None, None) => None,
        _ => {
            errors.push(format!(
                "{} and {}: set both or neither",
                var("TLS_CERT_FILE"),
                var("TLS_KEY_FILE")
            ));
            None
        }
    };

    (errors.len() == before).then(|| Route {
        name: name.to_ascii_lowercase(),
        hosts,
        path,
        strip_prefix,
        upstreams,
        set_host,
        cert_files,
    })
}

fn validate_host(host: &str, allow_wildcard: bool) -> Result<(), &'static str> {
    let name = match host.strip_prefix("*.") {
        Some(rest) if allow_wildcard => rest,
        Some(_) => return Err("may not be a wildcard"),
        None => host,
    };
    if name.is_empty() || name.len() > 253 {
        return Err("is not a valid host name");
    }
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    if !name.split('.').all(label_ok) {
        return Err("is not a valid host name (use host names only: no scheme, port or path)");
    }
    Ok(())
}

fn parse_upstream(item: &str) -> Result<Upstream, String> {
    let (tls, rest) = if let Some(r) = item.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = item.strip_prefix("http://") {
        (false, r)
    } else if item.contains("://") {
        return Err("has an unsupported scheme (use http:// or https://)".into());
    } else {
        (false, item)
    };
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.contains('/') {
        return Err("must not contain a path".into());
    }
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => {
            let port: u16 = p
                .parse()
                .map_err(|_| format!("has an invalid port '{p}'"))?;
            (h, port)
        }
        None if tls => (rest, 443),
        None => return Err("needs a port, like app:8080".into()),
    };
    // Underscores are not valid in DNS names but Docker Compose service names
    // routinely contain them, so they are accepted here.
    let ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
    if !ok {
        return Err("has an invalid host name (IPv6 upstreams are not supported)".into());
    }
    Ok(Upstream {
        host: host.to_ascii_lowercase(),
        port,
        tls,
    })
}

fn parse_addr(
    errors: &mut Vec<String>,
    key: &str,
    value: Option<String>,
    default: &str,
) -> SocketAddr {
    let fallback: SocketAddr = default.parse().expect("default address is valid");
    match value {
        None => fallback,
        Some(v) => v.parse().unwrap_or_else(|_| {
            errors.push(format!("{key}: '{v}' is not an address like 0.0.0.0:8080"));
            fallback
        }),
    }
}

fn parse_bool(errors: &mut Vec<String>, key: &str, value: Option<String>, default: bool) -> bool {
    match value.as_deref().map(str::to_ascii_lowercase).as_deref() {
        None => default,
        Some("true" | "1" | "yes" | "on") => true,
        Some("false" | "0" | "no" | "off") => false,
        Some(v) => {
            errors.push(format!("{key}: '{v}' is not true or false"));
            default
        }
    }
}

fn parse_num<T: std::str::FromStr>(
    errors: &mut Vec<String>,
    key: &str,
    value: Option<String>,
    default: T,
) -> T {
    match value {
        None => default,
        Some(v) => v.parse().unwrap_or_else(|_| {
            errors.push(format!("{key}: '{v}' is not a valid number"));
            default
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Result<Config, Vec<String>> {
        Config::from_env(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())))
    }

    #[test]
    fn minimal_config() {
        let cfg = env(&[
            ("ROUTE_APP_HOST", "Example.com, www.example.com"),
            ("ROUTE_APP_UPSTREAM", "app:8080"),
            ("PATH", "/usr/bin"),
        ])
        .unwrap();
        assert_eq!(cfg.tls, TlsMode::Off);
        assert!(!cfg.https_redirect);
        assert_eq!(cfg.routes.len(), 1);
        let r = &cfg.routes[0];
        assert_eq!(r.name, "app");
        assert_eq!(r.hosts, ["example.com", "www.example.com"]);
        assert_eq!(r.path, "/");
        assert_eq!(
            r.upstreams,
            [Upstream {
                host: "app".into(),
                port: 8080,
                tls: false
            }]
        );
    }

    #[test]
    fn route_names_may_contain_underscores() {
        let cfg = env(&[
            ("ROUTE_MY_API_HOST", "api.example.com"),
            (
                "ROUTE_MY_API_UPSTREAM",
                "https://backend.internal, http://other:81/",
            ),
            ("ROUTE_MY_API_PATH", "/v1/"),
            ("ROUTE_MY_API_STRIP_PREFIX", "true"),
            ("ROUTE_MY_API_TLS_CERT_FILE", "/c.pem"),
            ("ROUTE_MY_API_TLS_KEY_FILE", "/k.pem"),
            ("TLS_MODE", "manual"),
        ])
        .unwrap();
        let r = &cfg.routes[0];
        assert_eq!(r.name, "my_api");
        assert_eq!(r.path, "/v1");
        assert!(r.strip_prefix);
        assert_eq!(
            r.upstreams[0],
            Upstream {
                host: "backend.internal".into(),
                port: 443,
                tls: true
            }
        );
        assert_eq!(
            r.upstreams[1],
            Upstream {
                host: "other".into(),
                port: 81,
                tls: false
            }
        );
        assert!(r.cert_files.is_some());
        assert!(cfg.https_redirect);
    }

    #[test]
    fn reports_every_problem_at_once() {
        let errs = env(&[
            ("ROUTE_A_HOST", "https://example.com"),
            ("ROUTE_A_UPSTREAM", "app"),
            ("ROUTE_B_HOST", "b.example.com"),
            ("ROUTE_C_HOSTT", "typo.example.com"),
            ("TLS_MODE", "letsencrypt"),
            ("HTTP_ADDR", "80"),
        ])
        .unwrap_err();
        let all = errs.join("\n");
        assert!(all.contains("ROUTE_A_HOST"), "{all}");
        assert!(
            all.contains("ROUTE_A_UPSTREAM: 'app' needs a port"),
            "{all}"
        );
        assert!(all.contains("ROUTE_B_UPSTREAM: missing"), "{all}");
        assert!(
            all.contains("ROUTE_C_HOSTT: unknown route setting"),
            "{all}"
        );
        assert!(all.contains("TLS_MODE"), "{all}");
        assert!(all.contains("HTTP_ADDR"), "{all}");
    }

    #[test]
    fn no_routes_is_an_error() {
        let errs = env(&[("TLS_MODE", "off")]).unwrap_err();
        assert!(errs[0].contains("no routes configured"));
    }

    #[test]
    fn duplicate_host_and_path_is_an_error() {
        let errs = env(&[
            ("ROUTE_A_HOST", "example.com"),
            ("ROUTE_A_UPSTREAM", "a:1"),
            ("ROUTE_B_HOST", "example.com"),
            ("ROUTE_B_UPSTREAM", "b:1"),
        ])
        .unwrap_err();
        assert!(
            errs[0].contains("already served by another route"),
            "{errs:?}"
        );
    }

    #[test]
    fn acme_rejects_wildcards_without_cert_files() {
        let errs = env(&[
            ("TLS_MODE", "acme"),
            ("ROUTE_A_HOST", "*.example.com"),
            ("ROUTE_A_UPSTREAM", "a:1"),
        ])
        .unwrap_err();
        assert!(errs[0].contains("wildcard"), "{errs:?}");
    }

    #[test]
    fn acme_hosts_skip_routes_with_their_own_certificate() {
        let cfg = env(&[
            ("TLS_MODE", "acme"),
            ("ACME_STAGING", "true"),
            ("ROUTE_A_HOST", "a.example.com,b.example.com"),
            ("ROUTE_A_UPSTREAM", "a:1"),
            ("ROUTE_B_HOST", "*.corp.example.com"),
            ("ROUTE_B_UPSTREAM", "b:1"),
            ("ROUTE_B_TLS_CERT_FILE", "/c.pem"),
            ("ROUTE_B_TLS_KEY_FILE", "/k.pem"),
        ])
        .unwrap();
        assert_eq!(cfg.acme_hosts(), ["a.example.com", "b.example.com"]);
        assert_eq!(cfg.acme.directory_url, LETS_ENCRYPT_STAGING);
    }

    #[test]
    fn redirect_without_tls_is_an_error() {
        let errs = env(&[
            ("HTTPS_REDIRECT", "true"),
            ("ROUTE_A_HOST", "example.com"),
            ("ROUTE_A_UPSTREAM", "a:1"),
        ])
        .unwrap_err();
        assert!(errs[0].contains("HTTPS_REDIRECT"), "{errs:?}");
    }
}
