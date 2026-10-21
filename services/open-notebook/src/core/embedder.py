"""
Embedding service — local sentence-transformers with optional gateway fallback.
"""

import asyncio
from typing import List, Optional
from concurrent.futures import ThreadPoolExecutor

# Lazy-loaded sentence-transformers model
_model = None
_executor = ThreadPoolExecutor(max_workers=2)

DEFAULT_MODEL = "sentence-transformers/all-MiniLM-L6-v2"
EMBED_DIM = 384  # all-MiniLM-L6-v2 produces 384-dim vectors


def _get_model():
    """Lazy-load the embedding model."""
    global _model
    if _model is None:
        try:
            from sentence_transformers import SentenceTransformer
            model_name = DEFAULT_MODEL
            print(f"[Embedder] Loading local embedding model: {model_name}")
            _model = SentenceTransformer(model_name)
            print(f"[Embedder] Model loaded ({EMBED_DIM} dims)")
        except Exception as e:
            print(f"[Embedder] Failed to load sentence-transformers: {e}")
            raise
    return _model


def _embed_sync(texts: List[str]) -> List[List[float]]:
    """Synchronous embedding using sentence-transformers."""
    model = _get_model()
    embeddings = model.encode(texts, convert_to_numpy=True, normalize_embeddings=True)
    return embeddings.tolist()


class Embedder:
    async def embed(self, texts: List[str]) -> List[List[float]]:
        """Get embeddings via local sentence-transformers (fast, no external service)."""
        loop = asyncio.get_event_loop()
        return await loop.run_in_executor(_executor, _embed_sync, texts)


embedder = Embedder()
