"""Import-time production-safety checks for settings.

Kept in a standalone module (no Django settings access) so the logic is
unit-testable with plain parameters.
"""
from django.core.exceptions import ImproperlyConfigured

# The well-known dev fallback baked into settings.py. Fine for local dev,
# catastrophic in production (it also signs the JWTs).
DEV_SECRET_KEY = "dev-insecure-change-me-in-production-000000000000000000000000"


def validate_production_settings(debug: bool, secret_key: str) -> None:
    """Refuse to start a non-DEBUG deployment on a missing/dev SECRET_KEY.

    Called at settings import time; raises ImproperlyConfigured so the process
    dies loudly instead of serving traffic with a guessable signing key.
    """
    if debug:
        return
    if not secret_key or secret_key == DEV_SECRET_KEY:
        raise ImproperlyConfigured(
            "DEBUG=False but DJANGO_SECRET_KEY is unset or still the insecure "
            "dev default. Set DJANGO_SECRET_KEY to a long random value before "
            "deploying (it is both the Django secret and the JWT signing key)."
        )
