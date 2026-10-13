"""Notebook chunks go to allternit-api's memory plane; search asks it first.

A stub API (httpx.MockTransport) records calls; SurrealDB is replaced by an
in-memory fake so the retriever runs without a database or embedding model.
"""

import asyncio
import json
import sys
import types

import httpx
import pytest

from core.chunker import TextChunk
from core.memory_plane import MemoryPlane


class StubApi:
    def __init__(self, user_id="alice", fail_search=False, items=None):
        self.user_id = user_id
        self.fail_search = fail_search
        self.items = items or []
        self.calls = []

    def handler(self, request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content) if request.content else None
        self.calls.append((request.url.path, request.headers.get("authorization"), body))
        path = request.url.path
        if path == "/api/v1/me":
            return httpx.Response(200, json={"user": {"id": self.user_id}})
        if path.endswith("/chunks/upsert"):
            return httpx.Response(200, json={"created": len(body["items"])})
        if path.endswith("/chunks/delete"):
            return httpx.Response(200, json={"deleted": 1})
        if path.endswith("/chunks/search"):
            if self.fail_search:
                return httpx.Response(503, json={})
            return httpx.Response(200, json={"items": self.items})
        return httpx.Response(404)

    def plane(self, owners, **kw):
        async def owner_of(nb):
            return owners.get(nb)

        return MemoryPlane("http://api", "at-tok", owner_of=owner_of, transport=httpx.MockTransport(self.handler), **kw)

    def paths(self):
        return [c[0] for c in self.calls]


def chunks(nb, source="s1", n=2):
    return [TextChunk(f"chunk {i} of {source}", i, source, nb) for i in range(n)]


def run(coro):
    return asyncio.run(coro)


def test_upsert_scopes_by_token_and_notebook():
    api = StubApi()
    plane = api.plane({"nb1": "alice"})
    assert run(plane.upsert("nb1", chunks("nb1")))
    path, auth, body = api.calls[-1]
    assert path == "/api/v1/memory/adapters/chunks/upsert"
    assert auth == "Bearer at-tok"
    assert body["source"] == "open-notebook" and body["collection"] == "nb1"
    assert [i["external_id"] for i in body["items"]] == ["s1:0", "s1:1"]
    assert "user_id" not in json.dumps(body)


def test_never_mirrors_another_users_notebook():
    api = StubApi(user_id="alice")
    plane = api.plane({"nb-bob": "bob", "nb-free": None})
    assert not run(plane.upsert("nb-bob", chunks("nb-bob")))
    assert run(plane.search("q", "nb-bob")) is None
    assert not run(plane.delete("nb-bob"))
    assert "/api/v1/memory/adapters/chunks/upsert" not in api.paths()
    # Unowned notebooks: mirrored by default, not with mirror_unowned=False.
    assert run(plane.upsert("nb-free", chunks("nb-free")))
    strict = api.plane({"nb-free": None}, mirror_unowned=False)
    assert not run(strict.upsert("nb-free", chunks("nb-free")))


def test_disabled_without_token_and_tolerates_api_down():
    assert not MemoryPlane("http://api", "").enabled

    def down(request):
        raise httpx.ConnectError("refused")

    async def owner_of(nb):
        return None

    plane = MemoryPlane("http://api", "t", owner_of=owner_of, transport=httpx.MockTransport(down))
    assert not run(plane.upsert("nb1", chunks("nb1")))
    assert run(plane.search("q", "nb1")) is None


def test_search_maps_api_hits_to_retriever_shape():
    api = StubApi(items=[{"external_id": "s1:3", "document_id": "mdoc_ext_x", "text": "the passage", "score": 0.03}])
    plane = api.plane({"nb1": "alice"})
    hits = run(plane.search("passage", "nb1", limit=4))
    assert hits == [{
        "id": "s1:3", "text": "the passage", "source_id": "s1", "score": 0.03,
        "metadata": {"memory_plane": True, "document_id": "mdoc_ext_x"},
    }]
    assert api.calls[-1][2] == {"query": "passage", "source": "open-notebook", "collection": "nb1", "limit": 4}


class FakeDb:
    def __init__(self):
        self.rows = []
        self.notebooks = {"nb1": {"owner_id": "alice"}}

    async def create(self, table, data):
        self.rows.append(dict(data))
        return data

    async def select(self, table, id=None):
        return self.notebooks.get(id)

    async def query(self, sql, vars=None):
        vars = vars or {}
        if sql.startswith("SELECT notebook_id FROM chunk"):
            return [r for r in self.rows if r["source_id"] == vars["source_id"]]
        if sql.startswith("SELECT * FROM chunk"):
            return [r for r in self.rows if r["notebook_id"] == vars["notebook_id"]]
        if sql.startswith("DELETE FROM chunk WHERE source_id"):
            self.rows = [r for r in self.rows if r["source_id"] != vars["source_id"]]
        elif sql.startswith("DELETE FROM chunk WHERE notebook_id"):
            self.rows = [r for r in self.rows if r["notebook_id"] != vars["notebook_id"]]
        return []


@pytest.fixture
def retriever_mod(monkeypatch):
    fake = FakeDb()
    mod = types.ModuleType("db.surreal_client")
    mod.db = fake
    monkeypatch.setitem(sys.modules, "db.surreal_client", mod)
    monkeypatch.delitem(sys.modules, "core.retriever", raising=False)
    import core.retriever as r

    async def embed(texts, input_type="document"):
        return [[1.0, 0.0] if "0" in t or input_type == "query" else [0.0, 1.0] for t in texts]

    monkeypatch.setattr(r.embedder, "embed", embed)
    monkeypatch.setattr(r, "QDRANT_AVAILABLE", False)
    return r, fake


def test_retriever_writes_through_and_searches_api_first(retriever_mod):
    r, fake = retriever_mod
    api = StubApi(items=[{"external_id": "s1:0", "document_id": "d", "text": "from the plane", "score": 0.1}])
    ret = r.Retriever(plane=api.plane({"nb1": "alice"}))
    run(ret.add_chunks(chunks("nb1")))
    assert len(fake.rows) == 2 and fake.rows[0]["index"] == 0
    assert "/api/v1/memory/adapters/chunks/upsert" in api.paths()
    assert run(ret.search("q", "nb1"))[0]["text"] == "from the plane"

    run(ret.clear_source("s1"))
    path, _, body = api.calls[-1]
    assert path.endswith("/chunks/delete") and body["external_ids"] == ["s1:0", "s1:1"]
    assert fake.rows == []


def test_retriever_falls_back_to_local_when_api_fails_or_is_empty(retriever_mod):
    r, fake = retriever_mod
    api = StubApi(fail_search=True)
    ret = r.Retriever(plane=api.plane({"nb1": "alice"}))
    run(ret.add_chunks(chunks("nb1")))
    hits = run(ret.search("q", "nb1", limit=1))
    assert hits[0]["text"] == "chunk 0 of s1"
    empty = r.Retriever(plane=StubApi(items=[]).plane({"nb1": "alice"}))
    assert run(empty.search("q", "nb1", limit=1))[0]["text"] == "chunk 0 of s1"
