# pingenv

A small reverse proxy and static file server built on
[Pingora](https://github.com/cloudflare/pingora), configured **only by
environment variables**, with certificate issuance and renewal built in. No
config file, no admin UI, no sidecar.

```sh
docker run -p 80:80 -p 443:443 -v pingenv-data:/data \
  -e PINGENV_TLS_MODE=acme \
  -e PINGENV_ROUTE_APP_HOST=app.example.com \
  -e PINGENV_ROUTE_APP_UPSTREAM=app:8080 \
  pingenv
```

See [`docker-compose.yml`](docker-compose.yml) for a fuller example.

Every setting starts with `PINGENV_`. An unknown `PINGENV_*` variable is an
error, so a typo stops the container instead of being silently ignored;
variables without the prefix are never read. A bad configuration prints every
problem at once and exits with status 2.

## Routes

A route is a group of variables sharing a name:
`PINGENV_ROUTE_<NAME>_<SETTING>`. Names use `A-Z`, `0-9` and `_`, and only
show up in logs. Each route either proxies to an upstream or serves files.

| `PINGENV_ROUTE_<NAME>_…` | Meaning |
|---|---|
| `HOST` | **Required.** Host names, comma separated. `*.example.com` matches exactly one extra label. |
| `PATH` | Path prefix, default `/`. `/api` matches `/api` and `/api/x`, not `/apix`. |
| `TLS_CERT_FILE`, `TLS_KEY_FILE` | PEM certificate chain and key for this route's hosts, instead of ACME. Read once at startup. |

An exact host beats a wildcard, and within a host the longest path prefix
wins. Requests for a host with no route get a 404.

### Proxying

| `PINGENV_ROUTE_<NAME>_…` | Meaning |
|---|---|
| `UPSTREAM` | `host:port`, comma separated for round robin. Prefix with `https://` to connect over TLS (port defaults to 443, certificate is verified). |
| `STRIP_PREFIX` | `true` removes the path prefix before forwarding. Default `false`. |
| `SET_HOST` | Replaces the `Host` header sent upstream. By default the client's is kept. |

### Serving files

| `PINGENV_ROUTE_<NAME>_…` | Meaning |
|---|---|
| `ROOT` | Absolute path of the directory to serve. The route's path prefix maps onto it: with `PATH=/docs`, `/docs/a.css` is `ROOT/a.css`. |
| `MEMORY_MAX_FILE` | Files up to this size are held in memory; larger ones are streamed from disk. Default `1MiB`; `0` serves everything from disk. |
| `FALLBACK` | File served when nothing matches, for single-page apps, e.g. `/index.html`. Only applies to paths whose last segment has no dot, so a missing `/app.js` is still a 404. |
| `CACHE_CONTROL` | `Cache-Control` header for this route. Default `no-cache` (browsers revalidate, and get a cheap 304). |

Sizes are a number of bytes, or use `KiB`/`MiB`/`GiB` (or `KB`/`MB`/`GB`).

**The in-memory copy is a snapshot taken at startup.** It is never re-checked,
so a file edited or deleted later keeps being served as it was until the
restart. That suits a site baked into an image. For a directory that changes
while running, set `MEMORY_MAX_FILE=0`. Files that were not in the snapshot
(too large, or added later) are always read from disk.

What it does and does not do:

- `index.html` is served for directories; a directory asked for without its
  trailing slash is redirected. There are **no directory listings**.
- Dotfiles and dot-directories are never served, so a stray `.env` or `.git`
  cannot leak. `.well-known` is the one exception.
- `..` in any encoding is refused, and symlinks are only followed if they
  stay inside the root.
- `GET` and `HEAD` only. `ETag`, `Last-Modified`, conditional requests and
  single byte ranges are supported.
- No compression and no content negotiation.

## Layouts

The same image covers three setups; only the variables differ.

- **Proxy only.** Every route has an `UPSTREAM`.
- **One container doing both.** Some routes have an `UPSTREAM`, others a
  `ROOT`. See [`docker-compose.yml`](docker-compose.yml).
- **Edge plus file server.** One container terminates TLS and proxies; a
  second one, with TLS off, serves the directory. The file server's `HOST`
  must be the same as the edge's, since the client's `Host` header is passed
  through. See [`docker-compose.split.yml`](docker-compose.split.yml).

The split costs one extra hop per file request. What it buys: the site can be
redeployed by restarting the file server alone, without dropping the edge's
connections or its other routes, and the container holding the certificates
has no access to the content.

## Global settings

| Variable | Default | Meaning |
|---|---|---|
| `PINGENV_TLS_MODE` | `off` | `off`: HTTP only. `acme`: certificates from an ACME CA. `selfsigned`: generated at startup, for local use. `manual`: every route brings its own files. |
| `PINGENV_HTTP_ADDR` | `0.0.0.0:80` | Plain HTTP listener. |
| `PINGENV_HTTPS_ADDR` | `0.0.0.0:443` | HTTPS listener (HTTP/1.1 and HTTP/2), unless TLS is off. |
| `PINGENV_HTTPS_REDIRECT` | `true` with TLS | Redirect HTTP requests for known hosts to HTTPS (308). |
| `PINGENV_HTTPS_REDIRECT_PORT` | `443` | Port used in the redirect, if HTTPS is published on another one. |
| `PINGENV_DATA_DIR` | `/data` | Where the ACME account and certificates are stored. |
| `PINGENV_ACME_EMAIL` | none | Contact address registered with the CA. |
| `PINGENV_ACME_STAGING` | `false` | Use Let's Encrypt's staging CA (untrusted certificates, generous limits). |
| `PINGENV_ACME_DIRECTORY_URL` | Let's Encrypt | Any other ACME CA. |
| `PINGENV_ACME_CA_FILE` | none | Extra root to trust when talking to the ACME server (private or test CAs). |
| `PINGENV_MEMORY_MAX_TOTAL` | `128MiB` | Cap on file bytes held in memory across all routes. Exceeding it fails at startup rather than under load. |
| `PINGENV_THREADS` | CPU count | Worker threads. |
| `PINGENV_SHUTDOWN_GRACE_SECONDS` | `5` | Time in-flight requests get to finish after `SIGTERM`. |
| `PINGENV_LOG_LEVEL` | `info` | `off`, `error`, `warn`, `info`, `debug`, `trace`. |

## Certificates

With `PINGENV_TLS_MODE=acme`, each host gets its own certificate through the
HTTP-01 challenge. Using this mode means you accept the CA's terms of service.

- The host name must resolve to this proxy and **port 80 must be reachable**
  from the internet.
- **The data directory must be a volume.** Without it every restart requests
  new certificates and you will hit the CA's rate limits.
- Until the first certificate arrives the host serves a self-signed
  placeholder. Certificates are swapped in live, without a restart.
- Renewal happens after two thirds of a certificate's lifetime. Failures are
  retried after 15 minutes, backing off to 12 hours.
- Wildcard hosts cannot be validated over HTTP-01; give those routes
  certificate files.
- Start with `PINGENV_ACME_STAGING=true` and remove it once issuance works.

## Behaviour worth knowing

- `X-Forwarded-For`, `X-Forwarded-Proto` and `X-Forwarded-Host` are
  **overwritten**, not appended to: pingenv assumes it is the edge. Don't put
  it behind another proxy if your app relies on those.
- Upstream names are re-resolved every 10 seconds, so a container that returns
  with a new address is picked up.
- A failed upstream connection is retried on the next upstream (twice at
  most). There are no active health checks.
- TLS clients that send an unknown server name, or none, are refused.
- Access logs go to stderr, one line per request, without the query string.
  `upstream=` shows the upstream address, or `memory` / `disk` for files.
- The container runs as an unprivileged user (uid 65532). A bind mount used as
  the data directory has to be writable by it.
- On Kubernetes, a Service named `pingenv` makes the cluster inject
  `PINGENV_SERVICE_HOST`, `PINGENV_PORT` and similar into pods, which are
  rejected as unknown settings. Set `enableServiceLinks: false` on the pod, or
  give the Service another name.

## Not included

DNS-01 and wildcard issuance, multiple replicas sharing certificates, config
reload without restart, health checks, rate limiting, response caching,
compression, directory listings, metrics.

## Development

```sh
cargo test                          # unit tests
tests/e2e.sh                        # real binary: routing, files, self-signed TLS
tests/e2e.sh --acme <pebble-dir>    # plus a real ACME issuance against Pebble
```

Pingora's rustls backend, used here so the build needs no OpenSSL, is marked
experimental upstream.

## License

MIT. Dependencies keep their own licenses; Pingora is Apache-2.0.
