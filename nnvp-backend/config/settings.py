"""Django settings for the NNVP backend."""
import os
from pathlib import Path

BASE_DIR = Path(__file__).resolve().parent.parent


def env_bool(name, default=False):
    val = os.environ.get(name)
    if val is None:
        return default
    return val.strip().lower() in ("1", "true", "yes", "on")


def env_list(name, default):
    """Comma-separated env var -> list of stripped non-empty entries."""
    val = os.environ.get(name)
    if val is None:
        return default
    return [item.strip() for item in val.split(",") if item.strip()]


# --- Core config (env-driven with sane dev defaults) ---
from config.checks import DEV_SECRET_KEY, validate_production_settings  # noqa: E402

SECRET_KEY = os.environ.get("DJANGO_SECRET_KEY", DEV_SECRET_KEY)
DEBUG = env_bool("DEBUG", True)

# Refuse to start in production mode on the dev secret (raises ImproperlyConfigured).
validate_production_settings(DEBUG, SECRET_KEY)

# Hosts: env-driven. Permissive default only in DEBUG; strict (deny all) when
# DEBUG=False and DJANGO_ALLOWED_HOSTS is unset, so prod must opt in explicitly.
ALLOWED_HOSTS = env_list("DJANGO_ALLOWED_HOSTS", ["*"] if DEBUG else [])

# CORS: comma-separated exact origins (scheme://host[:port]). "*" means any
# origin. Default is permissive ONLY in DEBUG; empty (no CORS headers) otherwise.
CORS_ALLOWED_ORIGINS = env_list("CORS_ALLOWED_ORIGINS", ["*"] if DEBUG else [])

# Anthropic proxy key (server-side only; never exposed to clients).
ANTHROPIC_API_KEY = os.environ.get("ANTHROPIC_API_KEY", "")

# Assistant proxy hardening:
# - Auth: JWT Bearer required by default; set ASSISTANT_ALLOW_ANONYMOUS=1 to
#   restore anonymous access (local dev only).
# - Throttle: "<requests>/<s|m|h|d>" per authenticated user (per client IP when
#   anonymous access is enabled). Uses the default cache backend below.
ASSISTANT_ALLOW_ANONYMOUS = env_bool("ASSISTANT_ALLOW_ANONYMOUS", False)
ASSISTANT_THROTTLE_RATE = os.environ.get("ASSISTANT_THROTTLE_RATE", "30/m")

# Cache backend used by the assistant throttle. LocMemCache is per-process:
# correct for a single worker; with multiple workers each process keeps its own
# counters, so point this at a shared backend (Redis/Memcached) in that case.
CACHES = {
    "default": {
        "BACKEND": "django.core.cache.backends.locmem.LocMemCache",
        "LOCATION": "nnvp-backend",
    }
}

# JWT settings
JWT_ALGORITHM = "HS256"
JWT_EXP_SECONDS = int(os.environ.get("JWT_EXP_SECONDS", str(60 * 60 * 24 * 7)))  # 7 days

INSTALLED_APPS = [
    "django.contrib.auth",
    "django.contrib.contenttypes",
    "accounts",
    "projects",
    "assistant",
]

MIDDLEWARE = [
    "assistant.cors.CorsMiddleware",  # env-driven CORS for the SPA (see assistant/cors.py)
    "django.middleware.common.CommonMiddleware",
]

ROOT_URLCONF = "config.urls"

TEMPLATES = [
    {
        "BACKEND": "django.template.backends.django.DjangoTemplates",
        "DIRS": [],
        "APP_DIRS": True,
        "OPTIONS": {"context_processors": []},
    },
]

WSGI_APPLICATION = "config.wsgi.application"
ASGI_APPLICATION = "config.asgi.application"

DATABASES = {
    "default": {
        "ENGINE": "django.db.backends.sqlite3",
        "NAME": os.environ.get("DJANGO_DB_PATH", str(BASE_DIR / "db.sqlite3")),
    }
}

AUTH_USER_MODEL = "accounts.User"

# Django's standard validator stack: enforced on /api/auth/register (422 with
# the validator messages on failure). Login is unaffected.
AUTH_PASSWORD_VALIDATORS = [
    {"NAME": "django.contrib.auth.password_validation.UserAttributeSimilarityValidator",
     "OPTIONS": {"user_attributes": ("email",)}},
    {"NAME": "django.contrib.auth.password_validation.MinimumLengthValidator",
     "OPTIONS": {"min_length": 8}},
    {"NAME": "django.contrib.auth.password_validation.CommonPasswordValidator"},
    {"NAME": "django.contrib.auth.password_validation.NumericPasswordValidator"},
]

LANGUAGE_CODE = "en-us"
TIME_ZONE = "UTC"
USE_I18N = True
USE_TZ = True

DEFAULT_AUTO_FIELD = "django.db.models.BigAutoField"
