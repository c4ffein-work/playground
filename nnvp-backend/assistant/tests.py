import json

from django.test import TestCase, override_settings

from assistant import client


class AssistantProxyTests(TestCase):
    def _post(self, payload):
        return self.client.post(
            "/api/assistant/messages",
            data=json.dumps(payload),
            content_type="application/json",
        )

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
