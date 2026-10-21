from datetime import datetime
from typing import List, Optional
from fastapi import APIRouter, HTTPException, Query
from uuid import uuid4

from db.surreal_client import db, extract_id
from models import Notebook, NotebookCreate, NotebookUpdate, NotebookShareRequest

router = APIRouter(prefix="/api/notebooks", tags=["notebooks"])


@router.get("", response_model=List[Notebook])
async def list_notebooks(owner_id: Optional[str] = Query(None, description="Filter by owner")):
    """List notebooks. If owner_id is provided, returns notebooks owned by or shared with that user."""
    if owner_id:
        # Notebooks owned by user
        owned = await db.query(
            "SELECT * FROM notebook WHERE owner_id = $owner_id",
            {"owner_id": owner_id},
        )
        owned_records = owned if owned and len(owned) > 0 else []

        # Notebooks shared with user
        shared = await db.query(
            "SELECT * FROM notebook WHERE $owner_id IN shared_with",
            {"owner_id": owner_id},
        )
        shared_records = shared if shared and len(shared) > 0 else []

        # Deduplicate by ID
        by_id = {}
        for r in owned_records + shared_records:
            nid = extract_id(r)
            by_id[nid] = r
        records = list(by_id.values())
    else:
        records = await db.select("notebook")

    return [Notebook(**{**r, "id": extract_id(r)}) for r in records]


@router.post("", response_model=Notebook)
async def create_notebook(data: NotebookCreate):
    now = datetime.utcnow().isoformat()
    record = await db.create("notebook", {
        "title": data.title,
        "description": data.description,
        "owner_id": data.owner_id,
        "shared_with": [],
        "source_count": 0,
        "token_count": 0,
        "created_at": now,
        "updated_at": now,
    })
    return Notebook(**{**record, "id": extract_id(record)})


@router.get("/{notebook_id}", response_model=Notebook)
async def get_notebook(notebook_id: str):
    record = await db.select("notebook", notebook_id)
    if not record:
        raise HTTPException(status_code=404, detail="Notebook not found")
    return Notebook(**{**record, "id": extract_id(record)})


@router.patch("/{notebook_id}", response_model=Notebook)
async def update_notebook(notebook_id: str, data: NotebookUpdate):
    record = await db.select("notebook", notebook_id)
    if not record:
        raise HTTPException(status_code=404, detail="Notebook not found")

    updates = {}
    if data.title is not None:
        updates["title"] = data.title
    if data.description is not None:
        updates["description"] = data.description
    updates["updated_at"] = datetime.utcnow().isoformat()

    updated = await db.update("notebook", notebook_id, updates)
    return Notebook(**{**updated, "id": extract_id(updated)})


@router.post("/{notebook_id}/share")
async def share_notebook(notebook_id: str, req: NotebookShareRequest):
    """Share a notebook with a user or agent."""
    record = await db.select("notebook", notebook_id)
    if not record:
        raise HTTPException(status_code=404, detail="Notebook not found")

    shared_with = record.get("shared_with", []) or []
    if req.user_id not in shared_with:
        shared_with.append(req.user_id)
        await db.update("notebook", notebook_id, {"shared_with": shared_with})

    return {"success": True, "shared_with": shared_with}


@router.post("/{notebook_id}/unshare")
async def unshare_notebook(notebook_id: str, req: NotebookShareRequest):
    """Remove a user or agent from shared access."""
    record = await db.select("notebook", notebook_id)
    if not record:
        raise HTTPException(status_code=404, detail="Notebook not found")

    shared_with = record.get("shared_with", []) or []
    if req.user_id in shared_with:
        shared_with.remove(req.user_id)
        await db.update("notebook", notebook_id, {"shared_with": shared_with})

    return {"success": True, "shared_with": shared_with}


@router.delete("/{notebook_id}")
async def delete_notebook(notebook_id: str):
    # Also delete associated sources, chunks, and messages
    sources = await db.query("SELECT * FROM source WHERE notebook_id = $nb", {"nb": notebook_id})
    for source in sources if sources else []:
        sid = extract_id(source)
        await db.delete("source", sid)

    await db.query("DELETE FROM chunk WHERE notebook_id = $nb", {"nb": notebook_id})
    await db.query("DELETE FROM message WHERE notebook_id = $nb", {"nb": notebook_id})

    await db.delete("notebook", notebook_id)
    return {"success": True}
