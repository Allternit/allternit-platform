"""Origin-restricted wrapper for the pinned PrismML Image Studio application."""

from fastapi.middleware.cors import CORSMiddleware
from backend.server import app

app.add_middleware(
    CORSMiddleware,
    allow_origins=[
        "http://127.0.0.1:3013",
        "http://localhost:3013",
        "https://ai.allternit.com",
    ],
    allow_credentials=False,
    allow_methods=["GET", "POST"],
    allow_headers=["Content-Type"],
)

__all__ = ["app"]
