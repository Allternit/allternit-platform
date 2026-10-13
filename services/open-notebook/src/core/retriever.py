import os
import uuid
from typing import List, Dict, Any, Optional
from .embedder import embedder
from .chunker import TextChunk
from .memory_plane import MemoryPlane, external_id
from db.surreal_client import db

try:
    from qdrant_client import QdrantClient
    from qdrant_client.models import Distance, VectorParams, PointStruct, Filter, FieldCondition, MatchValue
    QDRANT_AVAILABLE = True
except ImportError:
    QDRANT_AVAILABLE = False


async def _notebook_owner(notebook_id: str) -> Optional[str]:
    record = await db.select("notebook", notebook_id)
    if isinstance(record, list):
        record = record[0] if record else None
    return (record or {}).get("owner_id") or None


class Retriever:
    """Vector retriever with Qdrant HNSW indexing for fast approximate search.

    Allternit's memory plane (allternit-api, see core/memory_plane.py) is the
    shared store: chunks are also written there and search asks it first.
    Locally, falls back to SurrealDB + numpy cosine if Qdrant is unavailable.
    """

    def __init__(self, plane: Optional[MemoryPlane] = None):
        self._qdrant: Optional[QdrantClient] = None
        self._collection = "notebook_chunks"
        self._use_qdrant = False
        self._dim: Optional[int] = None
        self.plane = plane or MemoryPlane.from_env(owner_of=_notebook_owner)

    async def _init_qdrant(self):
        if self._qdrant is not None:
            return
        if not QDRANT_AVAILABLE:
            return

        # Use in-memory Qdrant (no external server needed)
        self._qdrant = QdrantClient(":memory:")
        self._use_qdrant = True

    async def _ensure_collection(self, dim: int):
        await self._init_qdrant()
        if not self._qdrant:
            return

        if self._dim == dim:
            return
        self._dim = dim

        # Check if collection exists
        collections = self._qdrant.get_collections().collections
        exists = any(c.name == self._collection for c in collections)

        if not exists:
            self._qdrant.create_collection(
                collection_name=self._collection,
                vectors_config=VectorParams(size=dim, distance=Distance.COSINE),
            )

    async def add_chunks(self, chunks: List[TextChunk]) -> None:
        if not chunks:
            return

        texts = [c.text for c in chunks]
        embeddings = await embedder.embed(texts)

        # Store in SurrealDB for persistence
        for chunk, embedding in zip(chunks, embeddings):
            await db.create("chunk", {
                "text": chunk.text,
                "source_id": chunk.source_id,
                "notebook_id": chunk.notebook_id,
                "index": chunk.index,
                "embedding": embedding,
                "metadata": chunk.metadata or {},
            })

        # Index in Qdrant for fast search
        if QDRANT_AVAILABLE:
            dim = len(embeddings[0])
            await self._ensure_collection(dim)
            if self._qdrant:
                points = []
                for chunk, embedding in zip(chunks, embeddings):
                    point_id = str(uuid.uuid5(uuid.NAMESPACE_URL, f"{chunk.notebook_id}/{chunk.source_id}/{chunk.index}"))
                    points.append(PointStruct(
                        id=point_id,
                        vector=embedding,
                        payload={
                            "text": chunk.text,
                            "source_id": chunk.source_id,
                            "notebook_id": chunk.notebook_id,
                            "metadata": chunk.metadata or {},
                        },
                    ))
                self._qdrant.upsert(
                    collection_name=self._collection,
                    points=points,
                )

        # One memory plane: mirror into allternit-api, per notebook.
        by_notebook: Dict[str, List[TextChunk]] = {}
        for chunk in chunks:
            by_notebook.setdefault(chunk.notebook_id, []).append(chunk)
        for notebook_id, group in by_notebook.items():
            await self.plane.upsert(notebook_id, group)

    async def search(self, query: str, notebook_id: str, limit: int = 10) -> List[Dict[str, Any]]:
        # The shared memory plane first; local when it is off, down, or has
        # nothing yet for this notebook (not backfilled).
        remote = await self.plane.search(query, notebook_id, limit)
        if remote:
            return remote

        query_embedding = (await embedder.embed([query], "query"))[0]

        # Try Qdrant first
        if QDRANT_AVAILABLE and self._qdrant and self._dim:
            try:
                results = self._qdrant.search(
                    collection_name=self._collection,
                    query_vector=query_embedding,
                    query_filter=Filter(
                        must=[FieldCondition(key="notebook_id", match=MatchValue(value=notebook_id))]
                    ),
                    limit=limit,
                    with_payload=True,
                )
                return [
                    {
                        "id": r.id,
                        "text": r.payload.get("text", ""),
                        "source_id": r.payload.get("source_id", ""),
                        "score": r.score,
                        "metadata": r.payload.get("metadata", {}),
                    }
                    for r in results
                ]
            except Exception as e:
                # Fall through to SurrealDB fallback
                pass

        # Fallback: load from SurrealDB and compute cosine similarity
        return await self._search_fallback(query_embedding, notebook_id, limit)

    async def _search_fallback(self, query_embedding: List[float], notebook_id: str, limit: int) -> List[Dict[str, Any]]:
        import numpy as np

        result = await db.query(
            "SELECT * FROM chunk WHERE notebook_id = $notebook_id",
            {"notebook_id": notebook_id},
        )
        chunks = []
        if result:
            for row in result:
                if isinstance(row, dict):
                    chunks.append({
                        "id": row.get("id", ""),
                        "text": row.get("text", ""),
                        "source_id": row.get("source_id", ""),
                        "embedding": row.get("embedding", []),
                        "metadata": row.get("metadata", {}),
                    })

        if not chunks:
            return []

        query_vec = np.array(query_embedding)
        scores = []
        for chunk in chunks:
            chunk_vec = np.array(chunk["embedding"])
            # Chunks embedded by an older model (e.g. 384-dim MiniLM) live in
            # another vector space: skip them until the source is re-ingested.
            if chunk_vec.shape != query_vec.shape:
                continue
            norm = np.linalg.norm(query_vec) * np.linalg.norm(chunk_vec)
            similarity = np.dot(query_vec, chunk_vec) / norm if norm > 0 else 0.0
            scores.append({
                "id": chunk["id"],
                "text": chunk["text"],
                "source_id": chunk["source_id"],
                "score": float(similarity),
                "metadata": chunk.get("metadata", {}),
            })

        scores.sort(key=lambda x: x["score"], reverse=True)
        return scores[:limit]

    async def clear_source(self, source_id: str) -> None:
        # Chunk indexes are 0..n-1 per source: name them in the plane before
        # the local rows go.
        rows = await db.query("SELECT notebook_id FROM chunk WHERE source_id = $source_id", {"source_id": source_id})
        per_notebook: Dict[str, int] = {}
        for row in rows or []:
            if isinstance(row, dict) and row.get("notebook_id"):
                per_notebook[row["notebook_id"]] = per_notebook.get(row["notebook_id"], 0) + 1
        for notebook_id, n in per_notebook.items():
            await self.plane.delete(notebook_id, [external_id(source_id, i) for i in range(n)])
        await db.query("DELETE FROM chunk WHERE source_id = $source_id", {"source_id": source_id})
        if QDRANT_AVAILABLE and self._qdrant:
            try:
                self._qdrant.delete(
                    collection_name=self._collection,
                    points_selector=Filter(
                        must=[FieldCondition(key="source_id", match=MatchValue(value=source_id))]
                    ),
                )
            except Exception:
                pass

    async def clear_notebook(self, notebook_id: str) -> None:
        await self.plane.delete(notebook_id)
        self.plane.forget(notebook_id)
        await db.query("DELETE FROM chunk WHERE notebook_id = $notebook_id", {"notebook_id": notebook_id})
        if QDRANT_AVAILABLE and self._qdrant:
            try:
                self._qdrant.delete(
                    collection_name=self._collection,
                    points_selector=Filter(
                        must=[FieldCondition(key="notebook_id", match=MatchValue(value=notebook_id))]
                    ),
                )
            except Exception:
                pass


retriever = Retriever()
