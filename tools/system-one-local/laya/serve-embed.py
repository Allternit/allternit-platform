"""Local OpenAI-compatible embeddings server for the Allternit memory index.

Serves POST /v1/embeddings (and GET /health) with a ModernBERT-family
embedding model, by default nomic-ai/modernbert-embed-base (Apache-2.0,
768-dim, mean pooling). allternit-api reaches it at ALLTERNIT_EMBED_URL.

Request:  {"model": "...", "input": "text" | ["text", ...],
           "input_type": "query" | "document"}   # input_type is optional
Response: OpenAI shape: {"object": "list", "model": ..., "data": [{"index", "embedding"}], "usage": {...}}

The model wants task prefixes ("search_query: " / "search_document: "); the
server adds them from input_type so callers stay model-agnostic.

Env: EMBED_MODEL (repo id or local path), EMBED_REVISION (pinned HF revision),
     EMBED_HOST (127.0.0.1), EMBED_PORT (7719), EMBED_DEVICE (mps/cuda/cpu, auto),
     EMBED_MAX_TOKENS (512), EMBED_BATCH (32).
Only needs torch + transformers + fastapi + uvicorn (all already in the Laya venv).
"""
import os
import time

import torch
import torch.nn.functional as F
import uvicorn
from fastapi import FastAPI, HTTPException
from pydantic import BaseModel
from transformers import AutoModel, AutoTokenizer

MODEL_ID = os.environ.get("EMBED_MODEL", "nomic-ai/modernbert-embed-base")
REVISION = os.environ.get("EMBED_REVISION") or None
MAX_TOKENS = int(os.environ.get("EMBED_MAX_TOKENS", "512"))
BATCH = int(os.environ.get("EMBED_BATCH", "32"))
PREFIXES = {"query": "search_query: ", "document": "search_document: "}


def pick_device() -> str:
    dev = os.environ.get("EMBED_DEVICE")
    if dev:
        return dev
    if torch.cuda.is_available():
        return "cuda"
    if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available():
        return "mps"
    return "cpu"


DEVICE = pick_device()
tokenizer = AutoTokenizer.from_pretrained(MODEL_ID, revision=REVISION)
model = AutoModel.from_pretrained(MODEL_ID, revision=REVISION).to(DEVICE).eval()
DIM = int(model.config.hidden_size)
app = FastAPI()


class EmbedRequest(BaseModel):
    input: str | list[str]
    model: str | None = None
    input_type: str | None = None
    encoding_format: str | None = None


@torch.inference_mode()
def embed(texts: list[str]) -> tuple[list[list[float]], int]:
    out: list[list[float]] = []
    tokens = 0
    for i in range(0, len(texts), BATCH):
        batch = tokenizer(texts[i : i + BATCH], padding=True, truncation=True,
                          max_length=MAX_TOKENS, return_tensors="pt").to(DEVICE)
        tokens += int(batch["attention_mask"].sum())
        hidden = model(**batch).last_hidden_state
        mask = batch["attention_mask"].unsqueeze(-1).to(hidden.dtype)
        pooled = (hidden * mask).sum(1) / mask.sum(1).clamp(min=1e-9)
        out.extend(F.normalize(pooled, p=2, dim=1).float().cpu().tolist())
    return out, tokens


@app.get("/health")
@app.get("/healthz")
def health():
    return {"status": "ok", "model": MODEL_ID, "dim": DIM, "device": DEVICE}


@app.post("/v1/embeddings")
def embeddings(req: EmbedRequest):
    texts = [req.input] if isinstance(req.input, str) else list(req.input)
    if not texts:
        raise HTTPException(400, "input must not be empty")
    prefix = PREFIXES.get((req.input_type or "document").lower(), PREFIXES["document"])
    t0 = time.time()
    vectors, tokens = embed([prefix + t for t in texts])
    return {
        "object": "list",
        "model": MODEL_ID,
        "data": [{"object": "embedding", "index": i, "embedding": v} for i, v in enumerate(vectors)],
        "usage": {"prompt_tokens": tokens, "total_tokens": tokens},
        "latency_ms": int((time.time() - t0) * 1000),
    }


if __name__ == "__main__":
    uvicorn.run(app, host=os.environ.get("EMBED_HOST", "127.0.0.1"),
                port=int(os.environ.get("EMBED_PORT", "7719")), log_level="warning")
