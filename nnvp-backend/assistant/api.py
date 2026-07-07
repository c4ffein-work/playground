"""LLM proxy: forwards to the Anthropic Messages API injecting the server-side key.

Hardened:
- Requires a valid JWT Bearer token by default (401 otherwise). Set
  ASSISTANT_ALLOW_ANONYMOUS=1 to restore anonymous access for local dev.
- Rate-limited per identity via AssistantRateThrottle (429 when exceeded);
  the rate comes from ASSISTANT_THROTTLE_RATE (default 30/m).
"""
from django.conf import settings
from django.http import JsonResponse
from ninja import Router

from accounts.auth import JWTAuth
from assistant import client
from assistant.schemas import MessagesIn
from assistant.throttling import AssistantRateThrottle


class AssistantAuth(JWTAuth):
    """JWT Bearer auth with an env opt-out for anonymous local dev.

    Default: behaves exactly like JWTAuth (missing/invalid token -> 401).
    With ASSISTANT_ALLOW_ANONYMOUS enabled, unauthenticated requests are let
    through with a per-IP sentinel identity so the throttle still buckets per
    client instead of sharing one anonymous bucket.
    """

    def __call__(self, request):
        result = super().__call__(request)
        if result is None and settings.ASSISTANT_ALLOW_ANONYMOUS:
            return f"anon:{request.META.get('REMOTE_ADDR', 'unknown')}"
        return result


router = Router()


@router.post("/messages", auth=AssistantAuth(), throttle=[AssistantRateThrottle()])
def messages(request, data: MessagesIn):
    # Return a JsonResponse directly so Anthropic's status code passes straight
    # through without Ninja needing every possible upstream status pre-declared.
    api_key = settings.ANTHROPIC_API_KEY
    if not api_key:
        return JsonResponse(
            {
                "error": {
                    "type": "service_unavailable",
                    "message": "ANTHROPIC_API_KEY is not configured on the server.",
                }
            },
            status=503,
        )

    # Build the upstream payload, forwarding exactly what the client sent
    # (dropping keys the caller omitted so we don't send nulls upstream).
    payload = data.dict(exclude_none=True)

    # client.call_anthropic is monkeypatched in tests; never hits network there.
    status, body = client.call_anthropic(payload, api_key)
    return JsonResponse(body, status=status)
