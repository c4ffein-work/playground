import json

from django.test import TestCase

from accounts.models import User
from accounts.tokens import create_token


class ProjectsTests(TestCase):
    def setUp(self):
        self.user_a = User.objects.create_user("a@x.com", "supersecret")
        self.user_b = User.objects.create_user("b@x.com", "supersecret")
        self.token_a = create_token(self.user_a)
        self.token_b = create_token(self.user_b)

    def _req(self, method, path, token=None, payload=None):
        headers = {}
        if token:
            headers["HTTP_AUTHORIZATION"] = f"Bearer {token}"
        kwargs = dict(**headers)
        if payload is not None:
            kwargs["data"] = json.dumps(payload)
            kwargs["content_type"] = "application/json"
        return getattr(self.client, method)(path, **kwargs)

    def test_auth_required(self):
        self.assertEqual(self._req("get", "/api/projects").status_code, 401)
        self.assertEqual(
            self._req("post", "/api/projects", payload={"name": "n", "graph": {}}).status_code,
            401,
        )

    def test_crud_flow(self):
        graph = {"nodes": [{"id": 1, "type": "Dense"}], "edges": []}
        r = self._req("post", "/api/projects", self.token_a, {"name": "M1", "graph": graph})
        self.assertEqual(r.status_code, 201, r.content)
        pid = r.json()["id"]
        self.assertEqual(r.json()["graph"], graph)
        self.assertIn("updated_at", r.json())

        # list omits the graph blob
        r = self._req("get", "/api/projects", self.token_a)
        self.assertEqual(r.status_code, 200)
        items = r.json()
        self.assertEqual(len(items), 1)
        self.assertEqual(set(items[0].keys()), {"id", "name", "updated_at"})

        # detail includes graph
        r = self._req("get", f"/api/projects/{pid}", self.token_a)
        self.assertEqual(r.status_code, 200)
        self.assertEqual(r.json()["graph"], graph)

        # update
        r = self._req("put", f"/api/projects/{pid}", self.token_a, {"name": "M1-renamed"})
        self.assertEqual(r.status_code, 200)
        self.assertEqual(r.json()["name"], "M1-renamed")
        self.assertEqual(r.json()["graph"], graph)  # graph untouched

        new_graph = {"nodes": [], "edges": []}
        r = self._req("put", f"/api/projects/{pid}", self.token_a, {"graph": new_graph})
        self.assertEqual(r.status_code, 200)
        self.assertEqual(r.json()["graph"], new_graph)

        # delete
        r = self._req("delete", f"/api/projects/{pid}", self.token_a)
        self.assertEqual(r.status_code, 204)
        r = self._req("get", f"/api/projects/{pid}", self.token_a)
        self.assertEqual(r.status_code, 404)

    def test_ownership_isolation(self):
        r = self._req("post", "/api/projects", self.token_a, {"name": "secret", "graph": {}})
        pid = r.json()["id"]

        # user B cannot read/update/delete user A's project -> 404
        self.assertEqual(self._req("get", f"/api/projects/{pid}", self.token_b).status_code, 404)
        self.assertEqual(
            self._req("put", f"/api/projects/{pid}", self.token_b, {"name": "x"}).status_code, 404
        )
        self.assertEqual(
            self._req("delete", f"/api/projects/{pid}", self.token_b).status_code, 404
        )

        # and B's list stays empty
        self.assertEqual(self._req("get", "/api/projects", self.token_b).json(), [])
