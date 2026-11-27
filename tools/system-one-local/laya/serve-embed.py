"""Local OpenAI-compatible embeddings server for the Allternit memory index.

Serves POST /v1/embeddings (and GET /health) with Google's EmbeddingGemma 2
(google/embeddinggemma-2, Apache-2.0, 768-dim, Matryoshka 512/256/128,
8K context). allternit-api, the memory agent, open-notebook and gizzi's brain
all reach it at ALLTERNIT_EMBED_URL, so every Allternit surface shares one
vector space.

Request:  {"model": "...", "input": "text" | ["text", ...],
           "input_type": "query" | "document" | <prompt name>}   # input_type is optional
Response: OpenAI shape: {"object": "list", "model": ..., "data": [{"index", "embedding"}], "usage": {...}}

The model wants task prefixes ("task: search result | query: " /
"title: none | text: "). The server adds them from input_type so callers stay
model-agnostic. Besides query/document, input_type may name any prompt the
model ships (CodeRetrieval, QuestionAnswering, Classification, Clustering,
SentenceSimilarity, ...). An unknown input_type is treated as a document.

Env: EMBED_MODEL (repo id or local path), EMBED_REVISION (pinned HF revision),
     EMBED_HOST (127.0.0.1), EMBED_PORT (7719), EMBED_DEVICE (mps/cuda/cpu, auto),
     EMBED_MAX_TOKENS (2048), EMBED_BATCH (32),
     EMBED_DIM (768; 512/256/128 truncate via Matryoshka and re-normalize).

Also serves POST /v1/rerank (local cross-encoder, default BAAI/bge-reranker-v2-m3):
  Request:  {"query": "...", "documents": ["...", ...], "top_n": 12}     # top_n optional
  Response: {"model": ..., "results": [{"index": 3, "relevance_score": 0.91}, ...]}  # best first
Loads on the first call (thread-safe). At most 256 documents, each cut to 2,048 chars.
Disabled (503) when RAM < 8 GB or RERANK_DISABLE=1; /health reports
"rerank": {"model", "loaded", "enabled"}.
Env: RERANK_MODEL, RERANK_REVISION (pinned HF revision), RERANK_DEVICE (default: EMBED device),
     RERANK_BATCH (32), RERANK_MAX_TOKENS (512), RERANK_DISABLE (1 = off).

Text-only: the vision and audio encoders are not loaded (270M effective).
Needs torch + sentence-transformers>=6.1 + transformers>=5.19 + fastapi + uvicorn.
"""
import os
import threading
import time

import torch
import uvicorn
from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse
from pydantic import BaseModel
from sentence_transformers import SentenceTransformer

MODEL_ID = os.environ.get("EMBED_MODEL", "google/embeddinggemma-2")
REVISION = os.environ.get("EMBED_REVISION") or None
MAX_TOKENS = int(os.environ.get("EMBED_MAX_TOKENS", "2048"))
BATCH = int(os.environ.get("EMBED_BATCH", "32"))
TRUNCATE_DIM = int(os.environ.get("EMBED_DIM", "0")) or None
# input_type aliases -> the model's prompt names.
ALIASES = {"query": "SearchQuery", "document": "Document", "code": "CodeRetrieval"}


def pick_device() -> str:
    dev = os.environ.get("EMBED_DEVICE")
    if dev:
        return dev
    if torch.cuda.is_available():
        return "cuda"
    if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available():
        return "mps"
    return "cpu"


def pick_dtype(device: str) -> torch.dtype:
    # EmbeddingGemma 2 overflows float16 (NaN / silently bad vectors): bf16 where
    # the hardware has it natively, float32 everywhere else.
    if device == "cuda" and torch.cuda.is_bf16_supported():
        return torch.bfloat16
    return torch.float32


DEVICE = pick_device()
model = SentenceTransformer(
    MODEL_ID,
    revision=REVISION,
    device=DEVICE,
    truncate_dim=TRUNCATE_DIM,
    model_kwargs={"torch_dtype": pick_dtype(DEVICE)},
    config_kwargs={"vision_config": None, "audio_config": None},
)
model.max_seq_length = MAX_TOKENS
PROMPTS = dict(model.prompts or {})
DIM = int(model.get_embedding_dimension())
# Vectors from different dims never mix in the index (it keys on model + dim),
# but the reported id carries the dim so a truncated index is easy to spot.
SERVED_MODEL = MODEL_ID if not TRUNCATE_DIM else f"{MODEL_ID}@{DIM}"
app = FastAPI()

RERANK_MODEL = os.environ.get("RERANK_MODEL", "BAAI/bge-reranker-v2-m3")
RERANK_REVISION = os.environ.get("RERANK_REVISION") or None
RERANK_BATCH = int(os.environ.get("RERANK_BATCH", "32"))
RERANK_MAX_TOKENS = int(os.environ.get("RERANK_MAX_TOKENS", "512"))
RERANK_MAX_DOCS = 256
RERANK_MAX_CHARS = 2048
RERANK_MIN_RAM = 8 * 1024**3
_reranker = None
_rerank_lock = threading.Lock()


def total_ram() -> int | None:
    try:
        return os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
    except (ValueError, OSError, AttributeError):
        return None


def rerank_enabled() -> bool:
    if os.environ.get("RERANK_DISABLE", "").lower() in ("1", "true", "yes"):
        return False
    ram = total_ram()
    return ram is None or ram >= RERANK_MIN_RAM


def get_reranker():
    global _reranker
    with _rerank_lock:
        if _reranker is None:
            from sentence_transformers import CrossEncoder
            _reranker = CrossEncoder(
                RERANK_MODEL,
                revision=RERANK_REVISION,
                device=os.environ.get("RERANK_DEVICE") or DEVICE,
                max_length=RERANK_MAX_TOKENS,
            )
        return _reranker


class EmbedRequest(BaseModel):
    input: str | list[str]
    model: str | None = None
    input_type: str | None = None
    encoding_format: str | None = None


def prompt_for(input_type: str | None) -> str:
    name = ALIASES.get((input_type or "document").lower(), input_type or "Document")
    return PROMPTS.get(name, PROMPTS.get("Document", ""))


@torch.inference_mode()
def embed(texts: list[str], prompt: str) -> tuple[list[list[float]], int]:
    vectors = model.encode(texts, prompt=prompt, batch_size=BATCH,
                           normalize_embeddings=True, convert_to_numpy=True)
    tokens = sum(len(ids) for ids in model.tokenizer([prompt + t for t in texts])["input_ids"])
    return vectors.astype("float32").tolist(), tokens


@app.get("/health")
@app.get("/healthz")
def health():
    return {"status": "ok", "model": SERVED_MODEL, "dim": DIM, "device": DEVICE,
            "rerank": {"model": RERANK_MODEL, "loaded": _reranker is not None,
                       "enabled": rerank_enabled()}}


@app.post("/v1/embeddings")
def embeddings(req: EmbedRequest):
    texts = [req.input] if isinstance(req.input, str) else list(req.input)
    if not texts:
        raise HTTPException(400, "input must not be empty")
    t0 = time.time()
    vectors, tokens = embed(texts, prompt_for(req.input_type))
    return {
        "object": "list",
        "model": SERVED_MODEL,
        "data": [{"object": "embedding", "index": i, "embedding": v} for i, v in enumerate(vectors)],
        "usage": {"prompt_tokens": tokens, "total_tokens": tokens},
        "latency_ms": int((time.time() - t0) * 1000),
    }


class RerankRequest(BaseModel):
    query: str
    documents: list[str]
    top_n: int | None = None
    model: str | None = None


@app.post("/v1/rerank")
def rerank(req: RerankRequest):
    if not rerank_enabled():
        return JSONResponse(status_code=503, content={
            "error": "rerank disabled (RAM below 8 GB or RERANK_DISABLE=1)"})
    if not req.documents:
        raise HTTPException(400, "documents must not be empty")
    if len(req.documents) > RERANK_MAX_DOCS:
        raise HTTPException(400, f"at most {RERANK_MAX_DOCS} documents per request")
    docs = [d[:RERANK_MAX_CHARS] for d in req.documents]
    try:
        reranker = get_reranker()
        with torch.inference_mode():
            scores = reranker.predict([(req.query, d) for d in docs], batch_size=RERANK_BATCH,
                                      convert_to_numpy=True, show_progress_bar=False)
    except Exception as e:  # model download / load / OOM: caller falls back to its own order
        return JSONResponse(status_code=503, content={"error": f"rerank failed: {e}"})
    ranked = sorted(range(len(docs)), key=lambda i: float(scores[i]), reverse=True)
    if req.top_n and req.top_n > 0:
        ranked = ranked[:req.top_n]
    return {"model": RERANK_MODEL,
            "results": [{"index": i, "relevance_score": float(scores[i])} for i in ranked]}


if __name__ == "__main__":
    uvicorn.run(app, host=os.environ.get("EMBED_HOST", "127.0.0.1"),
                port=int(os.environ.get("EMBED_PORT", "7719")), log_level="warning")
