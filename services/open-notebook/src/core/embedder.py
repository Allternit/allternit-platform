"""
Embedding service — the shared Allternit embedding server (EmbeddingGemma 2),
with an in-process fallback to the same model.

Vectors come from ALLTERNIT_EMBED_URL (default http://127.0.0.1:7719, served by
tools/system-one-local/laya/serve-embed.sh), the endpoint allternit-api's
memory index and the memory agent use, so notebook chunks share one vector
space with the rest of Allternit. When the server is unreachable the model is
loaded locally with sentence-transformers instead (same model, same space).
"""

import asyncio
import os
from typing import List
from concurrent.futures import ThreadPoolExecutor

import httpx

DEFAULT_MODEL = "google/embeddinggemma-2"
EMBED_DIM = 768  # EmbeddingGemma 2 native dimension
EMBED_URL = os.getenv("ALLTERNIT_EMBED_URL", "http://127.0.0.1:7719").rstrip("/")
EMBED_MODEL = os.getenv("ALLTERNIT_EMBED_MODEL", DEFAULT_MODEL)
# Server input_type -> the model's prompt name, for the in-process fallback.
_PROMPTS = {"query": "SearchQuery", "document": "Document"}

# Lazy-loaded sentence-transformers model (fallback only)
_model = None
_executor = ThreadPoolExecutor(max_workers=2)


def _get_model():
    """Lazy-load the embedding model in-process (text encoder only)."""
    global _model
    if _model is None:
        from sentence_transformers import SentenceTransformer
        print(f"[Embedder] Loading local embedding model: {EMBED_MODEL}")
        _model = SentenceTransformer(
            EMBED_MODEL,
            config_kwargs={"vision_config": None, "audio_config": None},
        )
        print(f"[Embedder] Model loaded ({EMBED_DIM} dims)")
    return _model


def _embed_sync(texts: List[str], input_type: str) -> List[List[float]]:
    """Synchronous in-process embedding using sentence-transformers."""
    model = _get_model()
    embeddings = model.encode(
        texts,
        prompt_name=_PROMPTS.get(input_type, "Document"),
        convert_to_numpy=True,
        normalize_embeddings=True,
    )
    return embeddings.tolist()


class Embedder:
    model = EMBED_MODEL

    async def embed(self, texts: List[str], input_type: str = "document") -> List[List[float]]:
        """Embed via the shared server; fall back to the in-process model."""
        if EMBED_URL != "off":
            try:
                async with httpx.AsyncClient() as client:
                    response = await client.post(
                        f"{EMBED_URL}/v1/embeddings",
                        json={"model": EMBED_MODEL, "input": texts, "input_type": input_type},
                        timeout=60.0,
                    )
                    response.raise_for_status()
                    data = sorted(response.json()["data"], key=lambda d: d["index"])
                    return [d["embedding"] for d in data]
            except Exception as e:
                print(f"[Embedder] shared embedding server unavailable ({e}); using local model")
        loop = asyncio.get_event_loop()
        return await loop.run_in_executor(_executor, _embed_sync, texts, input_type)


embedder = Embedder()
