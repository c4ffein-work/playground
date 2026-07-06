"""Upstream Anthropic client. Isolated in one function so tests can monkeypatch it."""
import json
import urllib.error
import urllib.request

ANTHROPIC_URL = "https://api.anthropic.com/v1/messages"
ANTHROPIC_VERSION = "2023-06-01"


def call_anthropic(payload: dict, api_key: str, timeout: int = 60):
    """POST `payload` to the Anthropic Messages API, injecting the server-side key.

    Returns (status_code, response_json_dict). Monkeypatch this in tests so the
    network is never hit.
    """
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        ANTHROPIC_URL,
        data=body,
        method="POST",
        headers={
            "content-type": "application/json",
            "x-api-key": api_key,
            "anthropic-version": ANTHROPIC_VERSION,
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            status = resp.getcode()
            data = json.loads(resp.read().decode("utf-8"))
            return status, data
    except urllib.error.HTTPError as exc:
        # Pass Anthropic's error body + status straight through to the caller.
        raw = exc.read().decode("utf-8") if exc.fp else ""
        try:
            data = json.loads(raw) if raw else {"error": {"message": str(exc)}}
        except json.JSONDecodeError:
            data = {"error": {"message": raw or str(exc)}}
        return exc.code, data
