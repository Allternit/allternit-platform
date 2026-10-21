#!/usr/bin/env python3
"""
Android Bridge HTTP service.

Wraps the local phone-harness-android ADB harness so the Allternit
open-connector sidecar can send/read SMS and interact with a real Android
device over the network.

Endpoints:
  GET  /health
  POST /send-sms
  GET  /messages
  GET  /screenshot
  POST /tap
  POST /type
  POST /press-key

Environment:
  HARNESS_PATH - directory containing harness.py (defaults to ./phone-harness-android)
  PORT         - listen port (default 8020)
"""

from __future__ import annotations

import base64
import os
import sys
from contextlib import asynccontextmanager
from pathlib import Path
from typing import Any, Optional

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel, Field

HARNESS_DIR = Path(os.environ.get("HARNESS_PATH", "~/phone-harness-android")).resolve()
sys.path.insert(0, str(HARNESS_DIR))

try:
    import harness as android_harness  # type: ignore
except Exception as exc:  # pragma: no cover - harness may not be present in CI
    android_harness = None  # type: ignore
    HARNESS_LOAD_ERROR = str(exc)


def _ensure_harness() -> Any:
    if android_harness is None:
        raise HTTPException(status_code=503, detail=f"Android harness unavailable: {HARNESS_LOAD_ERROR}")
    return android_harness


app = FastAPI(title="Android Bridge")


class SendSmsRequest(BaseModel):
    to: str = Field(..., description="Recipient phone number")
    body: str = Field(..., description="Message body")


class TypeRequest(BaseModel):
    text: str = Field(..., description="Text to type")


class TapRequest(BaseModel):
    text: Optional[str] = Field(None, description="Tap the first OCR-matched text")
    x: Optional[int] = Field(None, description="X coordinate")
    y: Optional[int] = Field(None, description="Y coordinate")


class PressKeyRequest(BaseModel):
    key: str = Field(..., description="Key name (home, back, recent, power, menu)")


@app.get("/health")
def health() -> dict[str, Any]:
    ready = android_harness is not None and android_harness.check_device()
    return {"ok": True, "ready": ready, "harness_dir": str(HARNESS_DIR)}


@app.post("/send-sms")
def send_sms(payload: SendSmsRequest) -> dict[str, Any]:
    h = _ensure_harness()
    if not h.check_device():
        raise HTTPException(status_code=503, detail="No authorized Android device connected")

    try:
        h.press_home()
        h.wait_stable(0.5)
        # Open the default SMS app by launching the system dialer/messaging intent.
        h._adb(["shell", "am", "start", "-a", "android.intent.action.SENDTO", "-d", f"sms:{payload.to}"])
        h.wait_stable(1.0)
        # Focus the message body field and type.
        h._adb(["shell", "input", "keyevent", "61"])  # TAB to body on most SMS apps
        h._adb(["shell", "input", "text", payload.body.replace(" ", "%s")])
        h.wait_stable(0.5)
        # Tap the send button (fallback: press enter).
        h._adb(["shell", "input", "keyevent", "66"])
        return {"sent": True, "to": payload.to}
    except Exception as exc:
        raise HTTPException(status_code=500, detail=f"Failed to send SMS: {exc}") from exc


@app.get("/messages")
def list_messages(limit: int = 20) -> dict[str, Any]:
    h = _ensure_harness()
    if not h.check_device():
        raise HTTPException(status_code=503, detail="No authorized Android device connected")

    # Best-effort: dump SMS via content provider (requires root on some devices).
    result = h._adb(
        ["shell", "content", "query", "--uri", "content://sms/inbox", "--projection", "address:date:body"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise HTTPException(status_code=500, detail=f"Could not read SMS: {result.stderr}")

    messages: list[dict[str, Any]] = []
    for line in result.stdout.splitlines()[:limit]:
        # Each line looks like: Row: address=+123, date=123456789, body=hello
        try:
            parts = line.replace("Row: ", "").split(", ")
            record: dict[str, Any] = {}
            for part in parts:
                key, value = part.split("=", 1)
                record[key.strip()] = value.strip()
            messages.append(record)
        except Exception:
            continue

    return {"messages": messages}


@app.get("/screenshot")
def screenshot() -> dict[str, Any]:
    h = _ensure_harness()
    if not h.check_device():
        raise HTTPException(status_code=503, detail="No authorized Android device connected")

    path = "/tmp/android_bridge_screenshot.png"
    h.screenshot(path)
    data = Path(path).read_bytes()
    return {"image": base64.b64encode(data).decode("utf-8"), "format": "png"}


@app.post("/tap")
def tap(payload: TapRequest) -> dict[str, Any]:
    h = _ensure_harness()
    if not h.check_device():
        raise HTTPException(status_code=503, detail="No authorized Android device connected")

    try:
        if payload.text:
            path = "/tmp/android_bridge_tap.png"
            h.screenshot(path)
            ok = h.tap_text(payload.text, path)
            if not ok:
                raise HTTPException(status_code=404, detail=f"Text '{payload.text}' not found on screen")
            return {"tapped": True, "text": payload.text}
        if payload.x is not None and payload.y is not None:
            h._adb(["shell", "input", "tap", str(payload.x), str(payload.y)])
            return {"tapped": True, "x": payload.x, "y": payload.y}
        raise HTTPException(status_code=400, detail="Provide text or x/y coordinates")
    except HTTPException:
        raise
    except Exception as exc:
        raise HTTPException(status_code=500, detail=f"Tap failed: {exc}") from exc


@app.post("/type")
def type_text(payload: TypeRequest) -> dict[str, Any]:
    h = _ensure_harness()
    if not h.check_device():
        raise HTTPException(status_code=503, detail="No authorized Android device connected")

    try:
        escaped = payload.text.replace(" ", "%s")
        h._adb(["shell", "input", "text", escaped])
        return {"typed": True, "length": len(payload.text)}
    except Exception as exc:
        raise HTTPException(status_code=500, detail=f"Type failed: {exc}") from exc


@app.post("/press-key")
def press_key(payload: PressKeyRequest) -> dict[str, Any]:
    h = _ensure_harness()
    if not h.check_device():
        raise HTTPException(status_code=503, detail="No authorized Android device connected")

    key_map = {
        "home": "3",
        "back": "4",
        "recent": "187",
        "power": "26",
        "menu": "82",
    }
    code = key_map.get(payload.key.lower())
    if not code:
        raise HTTPException(status_code=400, detail=f"Unknown key: {payload.key}")

    h._adb(["shell", "input", "keyevent", code])
    return {"pressed": payload.key}


if __name__ == "__main__":
    import uvicorn

    port = int(os.environ.get("PORT", "8020"))
    uvicorn.run(app, host="127.0.0.1", port=port)
