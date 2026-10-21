import os
import uuid
from typing import List, Dict, Any, Optional
from .embedder import embedder
from .chunker import TextChunk
from db.surreal_client import db

try:
    from qdrant_client import QdrantClient
    from qdrant_client.models import Distance, VectorParams, PointStruct, Filter, FieldCondition, MatchValue
    QDRANT_AVAILABLE = True
except ImportError:
    QDRANT_AVAILABLE = False


class Retriever:
    """Vector retriever with Qdrant HNSW indexing for fast approximate search.

    Falls back to SurrealDB + numpy cosine if Qdrant is unavailable.
    """

    def __init__(self):
        self._qdrant: Optional[QdrantClient] = None
        self._collection = "notebook_chunks"
        self._use_qdrant = False
        self._dim: Optional[int] = None

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

    async def search(self, query: str, notebook_id: str, limit: int = 10) -> List[Dict[str, Any]]:
        query_embedding = (await embedder.embed([query]))[0]

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
