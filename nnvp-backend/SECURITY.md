# Security

Security posture of the NNVP backend: what is enforced, how to configure it,
and — honestly — what is still missing.

## Secrets handling

- **`DJANGO_SECRET_KEY`** is both the Django secret and the HS256 JWT signing
  key. It ships with a well-known insecure dev default that is only tolerated
  while `DEBUG=true`; with `DEBUG=false` the process **refuses to start**
  (`ImproperlyConfigured` at settings import, `config/checks.py`) if the key is
  unset or still the dev default.
  Generate one with: `python -c "import secrets; print(secrets.token_urlsafe(64))"`.
- **`ANTHROPIC_API_KEY`** is read from the environment only, used server-side
  only, and never appears in any response, log statement, or client-visible
  header. Keep it in a secret manager or the deployment environment — never in
  the repo, the Docker image, or the SPA bundle.
- No secrets are hard-coded anywhere in the codebase; `.env` is gitignored.

## HTTPS / reverse-proxy expectations

The app serves plain HTTP and expects to sit behind a TLS-terminating reverse
proxy (nginx, Caddy, a cloud load balancer):

- JWTs are Bearer tokens: anyone who captures one can replay it, so **never
  expose the API over plain HTTP** outside localhost.
- Set `DJANGO_ALLOWED_HOSTS` to your real hostname(s). When `DEBUG=false` and
  the variable is unset the default is **deny-all**, so production must opt in
  explicitly.
- The app does not currently set `SECURE_PROXY_SSL_HEADER`, HSTS, or redirect
  HTTP→HTTPS itself — do that at the proxy.
- Rate limiting identifies anonymous clients by `REMOTE_ADDR` /
  `X-Forwarded-For` (ninja's `get_ident`). If you run behind proxies, configure
  `NINJA_NUM_PROXIES` accordingly so clients can't spoof their identity via
  forged `X-Forwarded-For` headers.

## CORS

`assistant/cors.py` (dependency-free middleware) driven by
`CORS_ALLOWED_ORIGINS` (comma-separated exact origins, e.g.
`https://nnvp.example.com`):

- `DEBUG=true` + unset → `*` (any origin; dev convenience).
- `DEBUG=false` + unset → **no CORS headers**: browsers block cross-origin
  reads until you configure the SPA's origin.
- Explicitly listed origins are matched exactly and echoed back with
  `Vary: Origin`.

Remember CORS only constrains browsers — it is not authentication. Non-browser
clients can always call the API directly; that is what the JWT requirement and
throttling are for.

## The assistant proxy (`POST /api/assistant/messages`)

What it exposes: a passthrough to the Anthropic Messages API **billed to the
server's `ANTHROPIC_API_KEY`**. An unprotected proxy is effectively a free LLM
endpoint for the whole internet, so:

- **Auth required by default**: a valid JWT Bearer token, same as the rest of
  the API. Missing/invalid → 401. `ASSISTANT_ALLOW_ANONYMOUS=1` restores
  anonymous access for local development only — never set it in production.
- **Rate limited**: `ASSISTANT_THROTTLE_RATE` (default `30/m`) per
  authenticated user, per client IP in anonymous mode. Exceeding it → 429 with
  `Retry-After`.
- The key is injected server-side (`x-api-key`); clients never see it and
  client-supplied keys are ignored. Upstream status codes and error bodies pass
  through unchanged.
- The proxy forwards `model` and `max_tokens` as sent by the client. A hostile
  authenticated user can still pick your most expensive model and large
  outputs; add a server-side allowlist/cap if that matters for your budget
  (see gaps below). Also set a spend limit on the Anthropic key itself.

### Throttle caveat: single-process only by default

The throttle stores counters in Django's default cache, configured as
`LocMemCache` — **per process**. With one worker the limit is exact; with N
workers (gunicorn, etc.) each process keeps its own window, so the effective
limit can be up to N× the configured rate. For multi-worker deployments point
`CACHES["default"]` at a shared backend (Redis/Memcached). The limiter itself
(ninja's `SimpleRateThrottle`) works unchanged on top.

## Passwords

- Hashed with Django's default hasher (PBKDF2-SHA256).
- Registration enforces `AUTH_PASSWORD_VALIDATORS`: minimum length 8,
  common-password list, not-all-numeric, and not similar to the email. Weak
  passwords → **422** with the human-readable messages in `detail`.
- Login never reveals whether the email or the password was wrong (single 401).

## JWT expiry / rotation caveats

- Tokens are **stateless HS256** signed with `DJANGO_SECRET_KEY`, lifetime
  `JWT_EXP_SECONDS` (default 7 days).
- **No refresh tokens and no server-side revocation**: a stolen token is valid
  until it expires. Shorten `JWT_EXP_SECONDS` if that risk is unacceptable.
- **Rotating `DJANGO_SECRET_KEY` invalidates every outstanding token at once**
  (users must log in again). That is also the emergency kill switch if tokens
  or the key leak.
- Deactivating a user (`is_active=False`) takes effect immediately — the auth
  class reloads the user on every request.

## Known gaps (deliberate, documented)

- **No refresh tokens / revocation list** — logout is client-side only.
- **No email verification** — anyone can register any address they type.
- **No login/register rate limiting or lockout** — online password guessing is
  only slowed by the password validators; the throttle covers the assistant
  proxy only.
- **Single-process throttle by default** (see above).
- **No model/max_tokens allowlist on the proxy** — authenticated users choose
  what to spend.
- **No audit logging** of auth events or proxy usage.
- **SPA integration**: when the SPA's assistant base URL is set to this backend's
  URL, the client posts to `/api/assistant/messages` with the signed-in user's
  JWT (`Authorization: Bearer`) — no Anthropic key in the browser. Any other
  custom base URL is treated as a transparent Anthropic-compatible proxy
  (`/v1/messages` + `x-api-key`).

## Reporting

This is a playground project; open an issue or contact the maintainer directly
for anything sensitive.
