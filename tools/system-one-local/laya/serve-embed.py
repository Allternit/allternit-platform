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
Text-only: the vision and audio encoders are not loaded (270M effective).
Needs torch + sentence-transformers>=6.1 + transformers>=5.19 + fastapi + uvicorn.
"""
import os
import time

import torch
import uvicorn
from fastapi import FastAPI, HTTPException
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
    return {"status": "ok", "model": SERVED_MODEL, "dim": DIM, "device": DEVICE}


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


if __name__ == "__main__":
    uvicorn.run(app, host=os.environ.get("EMBED_HOST", "127.0.0.1"),
                port=int(os.environ.get("EMBED_PORT", "7719")), log_level="warning")
