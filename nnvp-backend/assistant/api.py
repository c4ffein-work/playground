"""LLM proxy: forwards to the Anthropic Messages API injecting the server-side key.

TODO: rate-limit / require auth. Auth is intentionally open for now.
"""
from django.conf import settings
from django.http import JsonResponse
from ninja import Router

from assistant import client
from assistant.schemas import MessagesIn

router = Router()


@router.post("/messages", auth=None)
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
