import json

from django.test import TestCase

from accounts.models import User


class AuthTests(TestCase):
    def _post(self, path, payload, token=None):
        headers = {}
        if token:
            headers["HTTP_AUTHORIZATION"] = f"Bearer {token}"
        return self.client.post(
            path, data=json.dumps(payload),
            content_type="application/json", **headers,
        )

    def _get(self, path, token=None):
        headers = {}
        if token:
            headers["HTTP_AUTHORIZATION"] = f"Bearer {token}"
        return self.client.get(path, **headers)

    def test_register_login_me_happy_path(self):
        r = self._post("/api/auth/register", {"email": "a@x.com", "password": "supersecret"})
        self.assertEqual(r.status_code, 201, r.content)
        body = r.json()
        self.assertIn("token", body)
        self.assertEqual(body["user"]["email"], "a@x.com")
        self.assertTrue(User.objects.filter(email="a@x.com").exists())

        r = self._post("/api/auth/login", {"email": "a@x.com", "password": "supersecret"})
        self.assertEqual(r.status_code, 200, r.content)
        token = r.json()["token"]

        r = self._get("/api/auth/me", token=token)
        self.assertEqual(r.status_code, 200, r.content)
        self.assertEqual(r.json()["email"], "a@x.com")

    def test_register_normalizes_email_case(self):
        self._post("/api/auth/register", {"email": "MixedCase@X.com", "password": "supersecret"})
        r = self._post("/api/auth/login", {"email": "mixedcase@x.com", "password": "supersecret"})
        self.assertEqual(r.status_code, 200, r.content)

    def test_login_bad_credentials(self):
        self._post("/api/auth/register", {"email": "b@x.com", "password": "supersecret"})
        r = self._post("/api/auth/login", {"email": "b@x.com", "password": "wrongpass"})
        self.assertEqual(r.status_code, 401)
        r = self._post("/api/auth/login", {"email": "nobody@x.com", "password": "whatever1"})
        self.assertEqual(r.status_code, 401)

    def test_me_without_token(self):
        r = self._get("/api/auth/me")
        self.assertEqual(r.status_code, 401)

    def test_me_with_invalid_token(self):
        r = self._get("/api/auth/me", token="not.a.valid.jwt")
        self.assertEqual(r.status_code, 401)

    def test_duplicate_registration_conflicts(self):
        self._post("/api/auth/register", {"email": "dup@x.com", "password": "supersecret"})
        r = self._post("/api/auth/register", {"email": "dup@x.com", "password": "supersecret"})
        self.assertEqual(r.status_code, 409)
