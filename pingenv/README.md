# pingenv

A small reverse proxy built on [Pingora](https://github.com/cloudflare/pingora),
configured **only by environment variables**, with certificate issuance and
renewal built in. No config file, no admin UI, no sidecar.

```sh
docker run -p 80:80 -p 443:443 -v pingenv-data:/data \
  -e TLS_MODE=acme \
  -e ROUTE_APP_HOST=app.example.com \
  -e ROUTE_APP_UPSTREAM=app:8080 \
  pingenv
```

See [`docker-compose.yml`](docker-compose.yml) for a fuller example.

## Routes

A route is a group of variables sharing a name: `ROUTE_<NAME>_<SETTING>`.
Names use `A-Z`, `0-9` and `_`, and only show up in logs.

| Variable | Required | Meaning |
|---|---|---|
| `ROUTE_<NAME>_HOST` | yes | Host names, comma separated. `*.example.com` matches exactly one extra label. |
| `ROUTE_<NAME>_UPSTREAM` | yes | `host:port`, comma separated for round robin. Prefix with `https://` to connect over TLS (port defaults to 443, certificate is verified). |
| `ROUTE_<NAME>_PATH` | no | Path prefix, default `/`. `/api` matches `/api` and `/api/x`, not `/apix`. |
| `ROUTE_<NAME>_STRIP_PREFIX` | no | `true` removes the path prefix before forwarding. Default `false`. |
| `ROUTE_<NAME>_SET_HOST` | no | Replaces the `Host` header sent upstream. By default the client's is kept. |
| `ROUTE_<NAME>_TLS_CERT_FILE`, `ROUTE_<NAME>_TLS_KEY_FILE` | no | PEM certificate chain and key for this route's hosts, instead of ACME. Read once at startup. |

An exact host beats a wildcard, and within a host the longest path prefix
wins. Requests for a host with no route get a 404.

## Global settings

| Variable | Default | Meaning |
|---|---|---|
| `TLS_MODE` | `off` | `off`: HTTP only. `acme`: certificates from an ACME CA. `selfsigned`: generated at startup, for local use. `manual`: every route brings its own files. |
| `HTTP_ADDR` | `0.0.0.0:80` | Plain HTTP listener. |
| `HTTPS_ADDR` | `0.0.0.0:443` | HTTPS listener (HTTP/1.1 and HTTP/2), unless `TLS_MODE=off`. |
| `HTTPS_REDIRECT` | `true` with TLS | Redirect HTTP requests for known hosts to HTTPS (308). |
| `HTTPS_REDIRECT_PORT` | `443` | Port used in the redirect, if HTTPS is published on another one. |
| `DATA_DIR` | `/data` | Where the ACME account and certificates are stored. |
| `ACME_EMAIL` | none | Contact address registered with the CA. |
| `ACME_STAGING` | `false` | Use Let's Encrypt's staging CA (untrusted certificates, generous limits). |
| `ACME_DIRECTORY_URL` | Let's Encrypt | Any other ACME CA. |
| `ACME_CA_FILE` | none | Extra root to trust when talking to the ACME server (private or test CAs). |
| `THREADS` | CPU count | Worker threads. |
| `SHUTDOWN_GRACE_SECONDS` | `5` | Time in-flight requests get to finish after `SIGTERM`. |
| `LOG_LEVEL` | `info` | `off`, `error`, `warn`, `info`, `debug`, `trace`. |

A bad configuration prints every problem at once and exits with status 2.

## Certificates

With `TLS_MODE=acme`, each host gets its own certificate through the HTTP-01
challenge. Using this mode means you accept the CA's terms of service.

- The host name must resolve to this proxy and **port 80 must be reachable**
  from the internet.
- **`DATA_DIR` must be a volume.** Without it every restart requests new
  certificates and you will hit the CA's rate limits.
- Until the first certificate arrives the host serves a self-signed
  placeholder. Certificates are swapped in live, without a restart.
- Renewal happens after two thirds of a certificate's lifetime. Failures are
  retried after 15 minutes, backing off to 12 hours.
- Wildcard hosts cannot be validated over HTTP-01; give those routes
  certificate files.
- Start with `ACME_STAGING=true` and remove it once issuance works.

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
- The container runs as an unprivileged user (uid 65532). A bind mount used as
  `DATA_DIR` has to be writable by it.

## Not included

DNS-01 and wildcard issuance, multiple replicas sharing certificates, config
reload without restart, health checks, rate limiting, caching, compression,
metrics.

## Development

```sh
cargo test                          # unit tests
tests/e2e.sh                        # real binary, routing and self-signed TLS
tests/e2e.sh --acme <pebble-dir>    # plus a real ACME issuance against Pebble
```

Pingora's rustls backend, used here so the build needs no OpenSSL, is marked
experimental upstream.

## License

MIT. Dependencies keep their own licenses; Pingora is Apache-2.0.
