"""Rate limiting for the assistant proxy, built on ninja's cache throttles.

Uses the default Django cache (LocMemCache in settings.py). LocMemCache is
per-process: exact for a single worker, but with N workers each process keeps
its own window, so the effective limit is up to N x the configured rate.
Point CACHES["default"] at a shared backend (Redis/Memcached) for multi-worker
deployments.
"""
from django.conf import settings
from ninja.throttling import AuthRateThrottle


class AssistantRateThrottle(AuthRateThrottle):
    """Per-identity throttle for the assistant endpoint.

    Keyed on str(request.auth): the User (its email) for JWT-authenticated
    requests, or the per-IP "anon:<ip>" sentinel set by AssistantAuth when
    ASSISTANT_ALLOW_ANONYMOUS is enabled.

    The rate is re-read from settings.ASSISTANT_THROTTLE_RATE on each request,
    so it stays env-configurable and can be overridden in tests.
    """

    scope = "assistant"

    def __init__(self):
        super().__init__(rate=settings.ASSISTANT_THROTTLE_RATE)

    def allow_request(self, request):
        rate = settings.ASSISTANT_THROTTLE_RATE
        if rate != self.rate:
            self.rate = rate
            self.num_requests, self.duration = self.parse_rate(rate)
        return super().allow_request(request)
