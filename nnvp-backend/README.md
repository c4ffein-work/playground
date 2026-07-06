# NNVP Backend

A standalone Django + Django Ninja REST backend for **NNVP** (a Keras model-graph
editor SPA). It provides JWT auth, per-user project storage (the SPA's saved model
graph), and a thin server-side proxy to the Anthropic Messages API.

This project is fully self-contained: its own venv, requirements, and settings. It
does **not** import or touch the SPA.

## Stack
- Django 5.1 + django-ninja 1.3 (single `NinjaAPI`, OpenAPI docs at `/api/docs`)
- PyJWT (HS256) for stateless Bearer auth
- SQLite for dev
- No external HTTP client dependency — the Anthropic proxy uses `urllib`

## Quick start

```bash
cd nnvp-backend
python3.11 -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt

cp .env.example .env        # optional; sane dev defaults exist without it

python manage.py migrate
python manage.py runserver 0.0.0.0:8000
```

Open http://localhost:8000/api/docs for the interactive OpenAPI UI.

## Environment variables

| Var                    | Default                    | Purpose                                                |
|------------------------|----------------------------|--------------------------------------------------------|
| `DJANGO_SECRET_KEY`    | insecure dev key           | Django secret + JWT signing key                        |
| `DEBUG`                | `true`                     | Django debug mode                                      |
| `DJANGO_ALLOWED_HOSTS` | `*`                        | Comma-separated allowed hosts                          |
| `JWT_EXP_SECONDS`      | `604800` (7 days)          | JWT lifetime                                           |
| `ANTHROPIC_API_KEY`    | *(unset)*                  | Server-side key for the assistant proxy; unset -> 503  |
| `DJANGO_DB_PATH`       | `./db.sqlite3`             | SQLite file location                                   |

Secrets are read from the environment only — nothing is hard-coded.

## Endpoints (base `/api`, JSON)

### Auth (`accounts`)
| Method | Path                 | Auth   | Body                | Success                              |
|--------|----------------------|--------|---------------------|--------------------------------------|
| POST   | `/api/auth/register` | none   | `{email, password}` | 201 `{token, user:{id,email}}`       |
| POST   | `/api/auth/login`    | none   | `{email, password}` | 200 `{token, user:{id,email}}` (401) |
| GET    | `/api/auth/me`       | Bearer | –                   | 200 `{id, email}` (401)              |

### Projects (`projects`, all require Bearer; users see only their own)
| Method | Path                  | Body            | Success                              |
|--------|-----------------------|-----------------|--------------------------------------|
| GET    | `/api/projects`       | –               | 200 `[{id,name,updated_at}]`         |
| POST   | `/api/projects`       | `{name, graph}` | 201 `{id,name,graph,updated_at}`     |
| GET    | `/api/projects/{id}`  | –               | 200 `{id,name,graph,updated_at}` (404 if not owner) |
| PUT    | `/api/projects/{id}`  | `{name?,graph?}`| 200 `{...}`                          |
| DELETE | `/api/projects/{id}`  | –               | 204                                  |

`graph` is an arbitrary JSON blob (the SPA's saved model). The list endpoint omits it.

### Assistant LLM proxy (`assistant`)
| Method | Path                     | Auth              | Body                                          |
|--------|--------------------------|-------------------|-----------------------------------------------|
| POST   | `/api/assistant/messages`| none *(for now)*  | `{model, max_tokens, messages, tools?, system?}` |

Forwards to `https://api.anthropic.com/v1/messages`, injecting the server-side
`x-api-key` and `anthropic-version: 2023-06-01`, and returns Anthropic's JSON +
status. Returns **503** if `ANTHROPIC_API_KEY` is unset. (TODO: rate-limit / require auth.)

## How JWT auth works
- On register/login the server issues a PyJWT token (HS256, signed with
  `DJANGO_SECRET_KEY`) carrying `sub` (user id), `email`, `iat`, and `exp`.
- Protected routes use a Ninja `HttpBearer` auth class (`accounts/auth.py`) that
  verifies the signature + expiry, loads the active user, and sets `request.auth`
  to the `User` instance. Invalid/missing tokens yield 401.
- Passwords are hashed with Django's auth (`set_password` / PBKDF2).

## CORS
A tiny permissive middleware (`assistant/cors.py`) sets `Access-Control-Allow-Origin: *`
so the SPA can call the API from any origin. Public-for-now; tighten before locking down.

## Tests

```bash
python manage.py test
```

Covers register→login→me plus auth failures, projects CRUD with ownership
isolation and auth-required checks, and the assistant proxy with the upstream
call monkeypatched (asserting body forwarding + key injection, and 503 when the
key is unset). The network is never hit in tests.

## Docker

```bash
docker build -t nnvp-backend .
docker run -p 8000:8000 -e ANTHROPIC_API_KEY=sk-... nnvp-backend
```
