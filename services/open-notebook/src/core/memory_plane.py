"""
Client for allternit-api's memory plane (one memory store for Allternit).

Notebook chunks are written into the API (`/api/v1/memory/adapters/chunks/*`,
source ``open-notebook``, collection = notebook id, external id =
``<source_id>:<chunk index>``), so the memory kernel and Agency runs recall
them, and notebook search asks the API first. SurrealDB + Qdrant stay the
local copy and the offline fallback.

Auth and scope: ``ALLTERNIT_API_TOKEN`` is a bearer credential of ONE user;
the API stores and searches in that user's scope only (it never reads a user
id from the request body). Open Notebook has no per-request identity, so only
notebooks that belong to the token's user are mirrored: notebooks whose
``owner_id`` is that user, or that have no owner (single-user installs;
``OPEN_NOTEBOOK_MIRROR_UNOWNED=0`` turns that off). A notebook owned by
someone else (even when shared with the token's user) is never written to,
nor searched through, the API.

Env: ``ALLTERNIT_API_URL`` (default http://127.0.0.1:8013),
``ALLTERNIT_API_TOKEN`` (unset = off), ``OPEN_NOTEBOOK_MEMORY_PLANE=0`` (off).
"""

import os
from typing import Any, Awaitable, Callable, Dict, List, Optional

import httpx

SOURCE = "open-notebook"
BATCH = 256
DEFAULT_API_URL = "http://127.0.0.1:8013"

OwnerLookup = Callable[[str], Awaitable[Optional[str]]]


def external_id(source_id: str, index: int) -> str:
    return f"{source_id}:{index}"


class MemoryPlane:
    def __init__(
        self,
        base_url: Optional[str],
        token: Optional[str],
        owner_of: Optional[OwnerLookup] = None,
        mirror_unowned: bool = True,
        transport: Optional[httpx.AsyncBaseTransport] = None,
        timeout: float = 10.0,
    ):
        self.base_url = (base_url or DEFAULT_API_URL).rstrip("/")
        self.token = (token or "").strip()
        self.owner_of = owner_of
        self.mirror_unowned = mirror_unowned
        self._transport = transport
        self._timeout = timeout
        self._user_id: Optional[str] = None
        self._allowed: Dict[str, bool] = {}

    @classmethod
    def from_env(cls, owner_of: Optional[OwnerLookup] = None) -> "MemoryPlane":
        token = os.getenv("ALLTERNIT_API_TOKEN", "")
        if os.getenv("OPEN_NOTEBOOK_MEMORY_PLANE", "1") == "0":
            token = ""
        return cls(
            os.getenv("ALLTERNIT_API_URL", DEFAULT_API_URL),
            token,
            owner_of=owner_of,
            mirror_unowned=os.getenv("OPEN_NOTEBOOK_MIRROR_UNOWNED", "1") != "0",
        )

    @property
    def enabled(self) -> bool:
        return bool(self.token)

    def _client(self) -> httpx.AsyncClient:
        return httpx.AsyncClient(
            base_url=self.base_url,
            headers={"Authorization": f"Bearer {self.token}"},
            timeout=self._timeout,
            transport=self._transport,
        )

    async def _post(self, path: str, body: Dict[str, Any]) -> Dict[str, Any]:
        async with self._client() as client:
            r = await client.post(path, json=body)
            r.raise_for_status()
            return r.json()

    async def user_id(self) -> Optional[str]:
        """The token's user (GET /api/v1/me), cached; None when unknown."""
        if self._user_id is None:
            try:
                async with self._client() as client:
                    r = await client.get("/api/v1/me")
                    r.raise_for_status()
                    self._user_id = (r.json().get("user") or {}).get("id") or None
            except Exception as e:
                print(f"[MemoryPlane] cannot resolve the API user ({e})")
                return None
        return self._user_id

    async def may_mirror(self, notebook_id: str) -> bool:
        """True when the notebook belongs to the token's user."""
        if not self.enabled or not notebook_id:
            return False
        if notebook_id in self._allowed:
            return self._allowed[notebook_id]
        me = await self.user_id()
        if me is None:
            return False  # not cached: retry once the API is reachable
        owner = await self.owner_of(notebook_id) if self.owner_of else None
        allowed = owner == me if owner else self.mirror_unowned
        self._allowed[notebook_id] = allowed
        return allowed

    def forget(self, notebook_id: str) -> None:
        self._allowed.pop(notebook_id, None)

    async def upsert(self, notebook_id: str, chunks: List[Any]) -> bool:
        """Write chunks (TextChunk-like: text, index, source_id) of one notebook."""
        if not chunks or not await self.may_mirror(notebook_id):
            return False
        try:
            for i in range(0, len(chunks), BATCH):
                items = [
                    {
                        "external_id": external_id(c.source_id, c.index),
                        "text": c.text,
                        "title": (c.metadata or {}).get("title"),
                    }
                    for c in chunks[i : i + BATCH]
                ]
                await self._post(
                    "/api/v1/memory/adapters/chunks/upsert",
                    {"source": SOURCE, "collection": notebook_id, "items": items},
                )
            return True
        except Exception as e:
            print(f"[MemoryPlane] upsert failed ({e}); notebook {notebook_id} stays local-only for now")
            return False

    async def delete(self, notebook_id: str, external_ids: Optional[List[str]] = None) -> bool:
        """Delete items of a notebook, or the whole notebook when ids is None."""
        if not await self.may_mirror(notebook_id):
            return False
        body: Dict[str, Any] = {"source": SOURCE, "collection": notebook_id}
        if external_ids is not None:
            if not external_ids:
                return True
            body["external_ids"] = external_ids
        try:
            await self._post("/api/v1/memory/adapters/chunks/delete", body)
            return True
        except Exception as e:
            print(f"[MemoryPlane] delete failed ({e})")
            return False

    async def search(self, query: str, notebook_id: str, limit: int = 10) -> Optional[List[Dict[str, Any]]]:
        """Hybrid search of one notebook in the API, in the retriever's result
        shape. None when the notebook is not mirrored or the API failed."""
        if not await self.may_mirror(notebook_id):
            return None
        try:
            data = await self._post(
                "/api/v1/memory/adapters/chunks/search",
                {"query": query, "source": SOURCE, "collection": notebook_id, "limit": limit},
            )
        except Exception as e:
            print(f"[MemoryPlane] search failed ({e}); using the local index")
            return None
        out = []
        for h in data.get("items", []):
            ext = h.get("external_id") or ""
            source_id = ext.rsplit(":", 1)[0] if ":" in ext else ext
            out.append({
                "id": ext or h.get("document_id", ""),
                "text": h.get("text", ""),
                "source_id": source_id,
                "score": h.get("score", 0.0),
                "metadata": {"memory_plane": True, "document_id": h.get("document_id")},
            })
        return out
