import json

from django.core.cache import cache
from django.test import TestCase, override_settings

from accounts.models import User
from accounts.tokens import create_token
from assistant import client


class AssistantTestCase(TestCase):
    """Base: an authenticated user + throttle-cache isolation between tests."""

    def setUp(self):
        cache.clear()  # throttle history lives in the (locmem) cache
        self.user = User.objects.create_user("assistant-user@x.com", "a-strong-passphrase")
        self.token = create_token(self.user)

    def tearDown(self):
        cache.clear()

    def _post(self, payload, token="DEFAULT"):
        headers = {}
        if token == "DEFAULT":
            token = self.token
        if token:
            headers["HTTP_AUTHORIZATION"] = f"Bearer {token}"
        return self.client.post(
            "/api/assistant/messages",
            data=json.dumps(payload),
            content_type="application/json",
            **headers,
        )


class AssistantProxyTests(AssistantTestCase):
    @override_settings(ANTHROPIC_API_KEY="server-secret-key")
    def test_forwards_body_and_injects_key(self):
        captured = {}

        def fake_call(payload, api_key, timeout=60):
            captured["payload"] = payload
            captured["api_key"] = api_key
            return 200, {"id": "msg_123", "content": [{"type": "text", "text": "hi"}]}

        original = client.call_anthropic
        client.call_anthropic = fake_call
        try:
            body = {
                "model": "claude-opus-4-8",
                "max_tokens": 128,
                "messages": [{"role": "user", "content": "hello"}],
                "system": "be brief",
            }
            r = self._post(body)
        finally:
            client.call_anthropic = original

        self.assertEqual(r.status_code, 200, r.content)
        self.assertEqual(r.json()["id"], "msg_123")
        # server-side key injected, never taken from the client
        self.assertEqual(captured["api_key"], "server-secret-key")
        # forwarded body preserved
        self.assertEqual(captured["payload"]["model"], "claude-opus-4-8")
        self.assertEqual(captured["payload"]["max_tokens"], 128)
        self.assertEqual(captured["payload"]["messages"], body["messages"])
        self.assertEqual(captured["payload"]["system"], "be brief")

    @override_settings(ANTHROPIC_API_KEY="server-secret-key")
    def test_passes_upstream_status_through(self):
        def fake_call(payload, api_key, timeout=60):
            return 400, {"error": {"type": "invalid_request_error", "message": "bad"}}

        original = client.call_anthropic
        client.call_anthropic = fake_call
        try:
            r = self._post({"model": "m", "max_tokens": 1, "messages": []})
        finally:
            client.call_anthropic = original

        self.assertEqual(r.status_code, 400)
        self.assertEqual(r.json()["error"]["type"], "invalid_request_error")

    @override_settings(ANTHROPIC_API_KEY="")
    def test_503_when_key_unset(self):
        called = {"hit": False}

        def fake_call(payload, api_key, timeout=60):
            called["hit"] = True
            return 200, {}

        original = client.call_anthropic
        client.call_anthropic = fake_call
        try:
            r = self._post({"model": "m", "max_tokens": 1, "messages": []})
        finally:
            client.call_anthropic = original

        self.assertEqual(r.status_code, 503)
        self.assertIn("ANTHROPIC_API_KEY", r.json()["error"]["message"])
        self.assertFalse(called["hit"])  # never called upstream without a key


class AssistantAuthTests(AssistantTestCase):
    """JWT is required by default; ASSISTANT_ALLOW_ANONYMOUS=1 opts out."""

    PAYLOAD = {"model": "m", "max_tokens": 1, "messages": []}

    @override_settings(ANTHROPIC_API_KEY="server-secret-key")
    def test_anonymous_request_is_401_by_default(self):
        called = {"hit": False}

        def fake_call(payload, api_key, timeout=60):
            called["hit"] = True
            return 200, {}

        original = client.call_anthropic
        client.call_anthropic = fake_call
        try:
            r = self._post(self.PAYLOAD, token=None)
        finally:
            client.call_anthropic = original

        self.assertEqual(r.status_code, 401)
        self.assertFalse(called["hit"])  # upstream never reached without auth

    @override_settings(ANTHROPIC_API_KEY="server-secret-key")
    def test_invalid_token_is_401(self):
        r = self._post(self.PAYLOAD, token="not.a.valid.jwt")
        self.assertEqual(r.status_code, 401)

    @override_settings(ANTHROPIC_API_KEY="server-secret-key", ASSISTANT_ALLOW_ANONYMOUS=True)
    def test_anonymous_allowed_with_env_optout(self):
        def fake_call(payload, api_key, timeout=60):
            return 200, {"id": "msg_anon"}

        original = client.call_anthropic
        client.call_anthropic = fake_call
        try:
            r = self._post(self.PAYLOAD, token=None)
        finally:
            client.call_anthropic = original

        self.assertEqual(r.status_code, 200, r.content)
        self.assertEqual(r.json()["id"], "msg_anon")


class AssistantThrottleTests(AssistantTestCase):
    """The proxy is rate-limited per identity (429 above the configured rate)."""

    @override_settings(ANTHROPIC_API_KEY="server-secret-key", ASSISTANT_THROTTLE_RATE="2/m")
    def test_exceeding_rate_returns_429(self):
        def fake_call(payload, api_key, timeout=60):
            return 200, {"id": "msg_ok"}

        original = client.call_anthropic
        client.call_anthropic = fake_call
        try:
            payload = {"model": "m", "max_tokens": 1, "messages": []}
            r1 = self._post(payload)
            r2 = self._post(payload)
            r3 = self._post(payload)

            # Another user has their own bucket and is not throttled.
            other = User.objects.create_user("other@x.com", "a-strong-passphrase")
            r_other = self._post(payload, token=create_token(other))
        finally:
            client.call_anthropic = original

        self.assertEqual(r1.status_code, 200, r1.content)
        self.assertEqual(r2.status_code, 200, r2.content)
        self.assertEqual(r3.status_code, 429, r3.content)
        self.assertEqual(r_other.status_code, 200, r_other.content)


class CorsMiddlewareTests(TestCase):
    """CORS headers are driven by settings.CORS_ALLOWED_ORIGINS."""

    @override_settings(CORS_ALLOWED_ORIGINS=["*"])
    def test_wildcard_when_configured_permissive(self):
        r = self.client.get("/api/docs")
        self.assertEqual(r["Access-Control-Allow-Origin"], "*")

    @override_settings(CORS_ALLOWED_ORIGINS=["https://app.example.com"])
    def test_explicit_origin_echoed_and_others_refused(self):
        r = self.client.get("/api/docs", HTTP_ORIGIN="https://app.example.com")
        self.assertEqual(r["Access-Control-Allow-Origin"], "https://app.example.com")
        self.assertIn("Origin", r.get("Vary", ""))

        r = self.client.get("/api/docs", HTTP_ORIGIN="https://evil.example.com")
        self.assertNotIn("Access-Control-Allow-Origin", r)

    @override_settings(CORS_ALLOWED_ORIGINS=[])
    def test_no_headers_when_unconfigured_strict(self):
        r = self.client.get("/api/docs", HTTP_ORIGIN="https://app.example.com")
        self.assertNotIn("Access-Control-Allow-Origin", r)
