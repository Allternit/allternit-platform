"""
One-shot backfill of existing notebook chunks into allternit-api's memory plane.

Re-chunks every source of every notebook the API token's user owns (or that
has no owner, unless OPEN_NOTEBOOK_MIRROR_UNOWNED=0) with the same chunker the
service uses, and upserts the chunks. Idempotent by id (source id + chunk
index): a re-run sends the same ids and the API reports them unchanged.

Usage (from services/open-notebook):
    ALLTERNIT_API_TOKEN=... [ALLTERNIT_API_URL=...] uv run python scripts/backfill_memory_plane.py
"""

import asyncio
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from core.chunker import chunk_text  # noqa: E402
from core.memory_plane import MemoryPlane  # noqa: E402
from db.surreal_client import db, extract_id  # noqa: E402


async def main() -> int:
    await db.connect()
    owners = {}

    async def owner_of(notebook_id: str):
        return owners.get(notebook_id)

    plane = MemoryPlane.from_env(owner_of=owner_of)
    if not plane.enabled:
        print("Set ALLTERNIT_API_TOKEN (and ALLTERNIT_API_URL if not the local API).")
        return 1
    if await plane.user_id() is None:
        print(f"Cannot reach {plane.base_url}/api/v1/me with this token.")
        return 1
    notebooks = await db.select("notebook") or []
    sent = skipped = failed = 0
    for nb in notebooks:
        nb_id = extract_id(nb)
        owners[nb_id] = nb.get("owner_id") or None
        if not await plane.may_mirror(nb_id):
            skipped += 1
            continue
        sources = await db.query("SELECT * FROM source WHERE notebook_id = $nb", {"nb": nb_id}) or []
        for src in sources:
            content = src.get("content") or ""
            if not content:
                continue
            chunks = chunk_text(content, extract_id(src), nb_id)
            if await plane.upsert(nb_id, chunks):
                sent += len(chunks)
            else:
                failed += 1
    await db.disconnect()
    print(f"backfill: {sent} chunks sent, {skipped} notebooks not owned by the token user, {failed} sources failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
