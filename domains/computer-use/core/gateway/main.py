"""
Allternit Computer Use Gateway - FastAPI with Persistent Sessions

Phase 3: Persistent browser sessions + core actions

Run: uvicorn main:app --host 127.0.0.1 --port 8760 --reload
     ACU_GATEWAY_PORT=8760 (override via env)
"""

from __future__ import annotations

import asyncio
import base64
import hashlib
import json
import logging
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, List, Literal, Optional

import os
from fastapi import FastAPI, HTTPException, Request, Response
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.middleware.cors import CORSMiddleware
from pydantic import BaseModel, Field
from uuid import uuid4

from session_manager import session_manager
from observability_integration import (
    init_observability,
    maybe_record_before,
    maybe_record_after,
    maybe_record_failure,
    finalize_run_observability,
)


logger = logging.getLogger(__name__)

# Tracks whether macOS Accessibility permissions are granted.
# Populated on startup; required for AX tree features to function.
_ax_permission_granted: bool = False

app = FastAPI(title="Allternit Computer Use Gateway", version="0.3.0")

_ACU_API_KEY = os.environ.get("ACU_API_KEY", "")

_AUTH_EXEMPT = {"/health", "/v1/computer-use/health"}


class _ApiKeyMiddleware(BaseHTTPMiddleware):
    async def dispatch(self, request: Request, call_next):
        if _ACU_API_KEY and request.url.path not in _AUTH_EXEMPT:
            auth = request.headers.get("Authorization", "")
            token = auth.removeprefix("Bearer ").strip()
            if token != _ACU_API_KEY:
                return Response(
                    content='{"detail":"Unauthorized"}',
                    status_code=401,
                    media_type="application/json",
                    headers={"WWW-Authenticate": "Bearer"},
                )
        return await call_next(request)


if _ACU_API_KEY:
    app.add_middleware(_ApiKeyMiddleware)

# This gateway is called directly from the browser (surfaces/ai.allternit.com's
# computer-use-engine.ts fetches its loopback port straight from the page),
# which makes every request cross-origin. Added here as an explicit, scoped
# fix rather than assuming it already worked -- there was no CORS middleware
# anywhere in this app before. Added after _ApiKeyMiddleware so it wraps
# outermost and handles preflight OPTIONS before the API-key check runs.
app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)


ActionType = Literal[
    "execute",
    "goto",
    "click",
    "fill",
    "extract",
    "screenshot",
    "inspect",
    "close",
    "scroll",
    "key",
    "type",
    "double_click",
    "right_click",
    "drag",
    "key_combo",
    "set_value",
    "toggle",
    "expand",
    "collapse",
    "hover",
    "triple_click",
    "get_clipboard",
    "set_clipboard",
    "find_elements",
    "ax_snapshot",
    # computer_20250124 / computer_20251124 native action set — the SDK
    # capability advertises these but they were previously unhandled here
    # and silently fell through to the stub.
    "mouse_move",
    "left_click",
    "left_click_drag",
    "middle_click",
    "left_mouse_down",
    "left_mouse_up",
    "cursor_position",
    "hold_key",
    "wait",
    "zoom",
]

AdapterPreference = Literal["playwright", "browser-use", "cdp", "desktop"]

FamilyType = Literal["browser", "desktop", "retrieval", "hybrid"]

ModeType = Literal[
    "assist",
    "execute",
    "inspect",
    "parallel",
    "desktop",
    "hybrid",
    "crawl",
]

StatusType = Literal["pending", "running", "completed", "failed", "cancelled"]


class ExecuteRequest(BaseModel):
    action: ActionType
    session_id: str
    run_id: str
    target: str | None = None
    goal: str | None = None
    text: str | None = None
    parameters: dict[str, Any] = Field(default_factory=dict)
    adapter_preference: AdapterPreference | None = None


class Artifact(BaseModel):
    type: Literal["screenshot", "download", "json", "text", "html"]
    path: str | None = None
    url: str | None = None
    mime: str | None = None
    content: str | None = None


class Receipt(BaseModel):
    action: str
    timestamp: str
    success: bool
    details: dict[str, Any] | None = None


class ErrorDetail(BaseModel):
    code: str
    message: str


class ExecuteResponse(BaseModel):
    run_id: str
    session_id: str
    adapter_id: str
    family: FamilyType
    mode: ModeType
    status: StatusType
    summary: str | None = None
    extracted_content: Any | None = None
    artifacts: list[Artifact] = Field(default_factory=list)
    receipts: list[Receipt] = Field(default_factory=list)
    error: ErrorDetail | None = None
    trace_id: str | None = None


class AppLaunchRequest(BaseModel):
    name: str
    bundle_id: Optional[str] = None


class AppCloseRequest(BaseModel):
    name: str


class WindowFocusRequest(BaseModel):
    window_id: Optional[int] = None
    title: Optional[str] = None
    app_name: Optional[str] = None


class WindowResizeRequest(BaseModel):
    window_id: int
    x: Optional[float] = None
    y: Optional[float] = None
    width: Optional[float] = None
    height: Optional[float] = None
    edge: Optional[str] = None  # resize by edge: top|bottom|left|right
    delta: Optional[float] = None


class WindowDragRequest(BaseModel):
    window_id: int
    delta_x: float
    delta_y: float


class NotificationActionRequest(BaseModel):
    action: str


class AXSnapshotRequest(BaseModel):
    surface: str = "window"
    skeleton: bool = False
    depth: int = -1
    interactive_only: bool = False
    window_id: Optional[int] = None


def utc_now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def infer_mode(req: ExecuteRequest) -> ModeType:
    if req.action == "inspect":
        return "inspect"
    return "execute"


def validate_request(req: ExecuteRequest) -> None:
    """Validate request parameters."""
    if req.text is not None and "text" not in req.parameters:
        req.parameters["text"] = req.text
    if req.action == "goto" and not req.target:
        raise HTTPException(status_code=400, detail="target is required for goto")
    if req.action == "click" and not req.target and not (req.parameters.get("x") is not None and req.parameters.get("y") is not None):
        raise HTTPException(status_code=400, detail="target or (x,y) coordinates required for click")
    if req.action == "fill":
        if not req.target:
            raise HTTPException(status_code=400, detail="target is required for fill")
        if not req.parameters.get("text"):
            raise HTTPException(
                status_code=400,
                detail="parameters.text is required for fill",
            )
    if req.action == "execute" and not req.goal:
        raise HTTPException(status_code=400, detail="goal is required for execute")


# ============================================================================
# Action Handlers - All wired with observability
# ============================================================================

async def handle_goto(req: ExecuteRequest) -> ExecuteResponse:
    """Navigate to URL using persistent session."""
    trace_id = str(uuid4())
    adapter_id = "browser.playwright"
    
    # Record frame start
    frame = await maybe_record_before(req)
    
    try:
        # Get or create session
        context, page = await session_manager.get_or_create_session(req.session_id)
        
        timeout = req.parameters.get("timeout", 30000)
        wait_until = req.parameters.get("wait_until", "domcontentloaded")
        
        # Navigate
        await page.goto(req.target, timeout=timeout, wait_until=wait_until)
        
        # Get page info
        url = page.url
        title = await page.title()
        
        # Capture after screenshot for observability
        after_screenshot = None
        try:
            after_screenshot = await page.screenshot(type="png")
        except Exception:
            pass  # Screenshot optional for observability
        
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="execute",
            status="completed",
            summary=f"Navigated to {url}",
            extracted_content={
                "url": url,
                "title": title,
            },
            artifacts=[],
            receipts=[
                Receipt(
                    action="goto",
                    timestamp=utc_now_iso(),
                    success=True,
                    details={
                        "target": req.target,
                        "url": url,
                        "title": title,
                    },
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame completion
        await maybe_record_after(frame, result, after_screenshot)
        
        return result
        
    except Exception as e:
        # Capture failure screenshot if possible
        failure_screenshot = None
        try:
            page = await session_manager.get_page(req.session_id)
            if page:
                failure_screenshot = await page.screenshot(type="png")
        except Exception:
            pass
        
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="execute",
            status="failed",
            summary=f"Failed to navigate to {req.target}",
            error=ErrorDetail(code="NAVIGATION_ERROR", message=str(e)),
            receipts=[
                Receipt(
                    action="goto",
                    timestamp=utc_now_iso(),
                    success=False,
                    details={"error": str(e)},
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame failure
        await maybe_record_failure(frame, e, failure_screenshot)
        
        return result


async def handle_screenshot(req: ExecuteRequest) -> ExecuteResponse:
    """Capture screenshot using persistent session."""
    trace_id = str(uuid4())
    adapter_id = "browser.playwright"
    
    # Record frame start
    frame = await maybe_record_before(req)
    
    try:
        # Get or create session
        context, page = await session_manager.get_or_create_session(req.session_id)
        
        # Optional: navigate first if target provided
        if req.target:
            timeout = req.parameters.get("timeout", 30000)
            await page.goto(req.target, timeout=timeout, wait_until="domcontentloaded")
        
        # Capture screenshot
        screenshot_bytes = await page.screenshot(
            full_page=req.parameters.get("full_page", False),
            type="png",
        )
        
        # Encode as base64 data URL
        b64_data = base64.b64encode(screenshot_bytes).decode("utf-8")
        data_url = f"data:image/png;base64,{b64_data}"
        
        url = page.url
        
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="inspect",
            status="completed",
            summary=f"Screenshot captured: {url}",
            artifacts=[
                Artifact(
                    type="screenshot",
                    mime="image/png",
                    url=data_url,
                )
            ],
            receipts=[
                Receipt(
                    action="screenshot",
                    timestamp=utc_now_iso(),
                    success=True,
                    details={
                        "url": url,
                        "size_bytes": len(screenshot_bytes),
                        "full_page": req.parameters.get("full_page", False),
                    },
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame completion (screenshot as after_screenshot)
        await maybe_record_after(frame, result, screenshot_bytes)
        
        return result
        
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="inspect",
            status="failed",
            summary="Failed to capture screenshot",
            error=ErrorDetail(code="SCREENSHOT_ERROR", message=str(e)),
            receipts=[
                Receipt(
                    action="screenshot",
                    timestamp=utc_now_iso(),
                    success=False,
                    details={"error": str(e)},
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame failure
        await maybe_record_failure(frame, e)
        
        return result


async def handle_click(req: ExecuteRequest) -> ExecuteResponse:
    """Click element on current page."""
    trace_id = str(uuid4())
    adapter_id = "browser.playwright"
    
    # Record frame start
    frame = await maybe_record_before(req)
    
    try:
        page = await session_manager.get_page(req.session_id)
        if not page:
            raise Exception("No active session. Use 'goto' first.")
        
        selector = req.target
        timeout = req.parameters.get("timeout", 5000)
        x = req.parameters.get("x")
        y = req.parameters.get("y")

        url_before = page.url

        if x is not None and y is not None:
            # Coordinate-based click — no selector needed
            await page.mouse.click(float(x), float(y))
        else:
            # Selector-based click
            await page.wait_for_selector(selector, timeout=timeout)
            await page.click(selector)
        
        # Wait a bit for potential navigation
        await asyncio.sleep(0.5)
        
        # Check if navigated
        url_after = page.url
        navigated = url_after != url_before
        
        # Capture after screenshot
        after_screenshot = None
        try:
            after_screenshot = await page.screenshot(type="png")
        except Exception:
            pass
        
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="execute",
            status="completed",
            summary=f"Clicked {selector}" + (f" and navigated to {url_after}" if navigated else ""),
            extracted_content={
                "navigated": navigated,
                "url_before": url_before,
                "url_after": url_after,
            },
            receipts=[
                Receipt(
                    action="click",
                    timestamp=utc_now_iso(),
                    success=True,
                    details={
                        "selector": selector,
                        "navigated": navigated,
                        "url": url_after,
                    },
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame completion
        await maybe_record_after(frame, result, after_screenshot)
        
        return result
        
    except Exception as e:
        # Capture failure screenshot if possible
        failure_screenshot = None
        try:
            page = await session_manager.get_page(req.session_id)
            if page:
                failure_screenshot = await page.screenshot(type="png")
        except Exception:
            pass
        
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="execute",
            status="failed",
            summary=f"Failed to click {req.target}",
            error=ErrorDetail(
                code="SELECTOR_ERROR" if "Timeout" in str(e) else "CLICK_ERROR",
                message=str(e)
            ),
            receipts=[
                Receipt(
                    action="click",
                    timestamp=utc_now_iso(),
                    success=False,
                    details={"selector": req.target, "error": str(e)},
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame failure
        await maybe_record_failure(frame, e, failure_screenshot)
        
        return result


async def handle_fill(req: ExecuteRequest) -> ExecuteResponse:
    """Fill input field on current page."""
    trace_id = str(uuid4())
    adapter_id = "browser.playwright"
    
    # Record frame start
    frame = await maybe_record_before(req)
    
    try:
        page = await session_manager.get_page(req.session_id)
        if not page:
            raise Exception("No active session. Use 'goto' first.")
        
        selector = req.target
        text = req.parameters.get("text")
        submit = req.parameters.get("submit", False)
        timeout = req.parameters.get("timeout", 5000)
        
        # Wait for element
        await page.wait_for_selector(selector, timeout=timeout)
        
        # Clear and fill
        await page.fill(selector, text)
        
        # Optional submit
        submitted = False
        if submit:
            await page.press(selector, "Enter")
            submitted = True
        
        # Capture after screenshot
        after_screenshot = None
        try:
            after_screenshot = await page.screenshot(type="png")
        except Exception:
            pass
        
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="execute",
            status="completed",
            summary=f"Filled {selector}" + (" and submitted" if submitted else ""),
            extracted_content={
                "selector": selector,
                "submitted": submitted,
            },
            receipts=[
                Receipt(
                    action="fill",
                    timestamp=utc_now_iso(),
                    success=True,
                    details={
                        "selector": selector,
                        "text_length": len(text),
                        "submitted": submitted,
                    },
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame completion
        await maybe_record_after(frame, result, after_screenshot)
        
        return result
        
    except Exception as e:
        # Capture failure screenshot if possible
        failure_screenshot = None
        try:
            page = await session_manager.get_page(req.session_id)
            if page:
                failure_screenshot = await page.screenshot(type="png")
        except Exception:
            pass
        
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="execute",
            status="failed",
            summary=f"Failed to fill {req.target}",
            error=ErrorDetail(
                code="SELECTOR_ERROR" if "Timeout" in str(e) else "FILL_ERROR",
                message=str(e)
            ),
            receipts=[
                Receipt(
                    action="fill",
                    timestamp=utc_now_iso(),
                    success=False,
                    details={"selector": req.target, "error": str(e)},
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame failure
        await maybe_record_failure(frame, e, failure_screenshot)
        
        return result


async def handle_extract(req: ExecuteRequest) -> ExecuteResponse:
    """Extract content from current page."""
    trace_id = str(uuid4())
    adapter_id = "browser.playwright"
    
    # Record frame start
    frame = await maybe_record_before(req)
    
    try:
        page = await session_manager.get_page(req.session_id)
        if not page:
            raise Exception("No active session. Use 'goto' first.")
        
        format_type = req.parameters.get("format", "text")
        selector = req.target  # Optional CSS selector
        
        if format_type == "text":
            if selector:
                # Extract text from specific element
                text = await page.text_content(selector)
            else:
                # Extract all text from page
                text = await page.evaluate("() => document.body.innerText")
            extracted = text
            
        elif format_type == "html":
            if selector:
                html = await page.inner_html(selector)
            else:
                html = await page.content()
            extracted = html
            
        elif format_type == "json":
            # Extract structured data (links, headings, etc.)
            data = await page.evaluate("""() => {
                return {
                    title: document.title,
                    url: window.location.href,
                    headings: Array.from(document.querySelectorAll('h1, h2, h3')).map(h => ({
                        level: h.tagName,
                        text: h.innerText.trim()
                    })),
                    links: Array.from(document.querySelectorAll('a[href]')).map(a => ({
                        text: a.innerText.trim(),
                        href: a.href
                    })).slice(0, 20),
                };
            }""")
            extracted = data
        else:
            extracted = None
        
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="inspect",
            status="completed",
            summary=f"Extracted {format_type} from page",
            extracted_content=extracted,
            receipts=[
                Receipt(
                    action="extract",
                    timestamp=utc_now_iso(),
                    success=True,
                    details={
                        "format": format_type,
                        "selector": selector,
                        "content_length": len(str(extracted)) if extracted else 0,
                    },
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame completion (no screenshot for extract)
        await maybe_record_after(frame, result)
        
        return result
        
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="inspect",
            status="failed",
            summary="Failed to extract content",
            error=ErrorDetail(code="EXTRACT_ERROR", message=str(e)),
            receipts=[
                Receipt(
                    action="extract",
                    timestamp=utc_now_iso(),
                    success=False,
                    details={"error": str(e)},
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame failure
        await maybe_record_failure(frame, e)
        
        return result


async def handle_inspect(req: ExecuteRequest) -> ExecuteResponse:
    """Inspect current page structure."""
    trace_id = str(uuid4())
    adapter_id = "browser.playwright"
    
    # Record frame start
    frame = await maybe_record_before(req)
    
    try:
        page = await session_manager.get_page(req.session_id)
        if not page:
            raise Exception("No active session. Use 'goto' first.")
        
        url = page.url
        title = await page.title()
        strategy = req.parameters.get("strategy", "accessibility")
        description = req.target or ""

        if strategy == "selector" and description:
            el = await page.query_selector(description)
            if el:
                bbox = await el.bounding_box()
                text = await el.inner_text()
                extracted = {"found": True, "selector": description, "text": text, "bounds": bbox}
            else:
                extracted = {"found": False, "selector": description}

        elif strategy == "text" and description:
            try:
                el = page.get_by_text(description).first
                bbox = await el.bounding_box()
                extracted = {"found": bbox is not None, "selector": f"text={description}", "text": description, "bounds": bbox}
            except Exception:
                extracted = {"found": False, "selector": f"text={description}"}

        elif strategy == "accessibility" and description:
            node = await page.evaluate("""label => {
                const wanted = label.toLowerCase();
                const roleFor = el => el.getAttribute('role') || ({
                    A: 'link', BUTTON: 'button', INPUT: 'textbox',
                    SELECT: 'combobox', TEXTAREA: 'textbox', IMG: 'img'
                }[el.tagName] || el.tagName.toLowerCase());
                for (const el of document.querySelectorAll('body *')) {
                    const name = (el.getAttribute('aria-label') || el.innerText ||
                        el.getAttribute('alt') || el.getAttribute('title') ||
                        el.getAttribute('placeholder') || el.value || '').trim();
                    if (name.toLowerCase().includes(wanted)) {
                        return {name: name.slice(0, 500), role: roleFor(el)};
                    }
                }
                return null;
            }""", description)
            extracted = {"found": node is not None, "selector": description, "text": node.get("name") if node else None, "role": node.get("role") if node else None}

        else:
            # Default: full DOM structure summary
            extracted = await page.evaluate("""() => ({
                title: document.title,
                url: window.location.href,
                meta: { description: document.querySelector('meta[name="description"]')?.content || '' },
                headings: Array.from(document.querySelectorAll('h1,h2,h3')).map(h => ({ level: h.tagName, text: h.innerText.trim().slice(0, 100) })),
                forms: Array.from(document.querySelectorAll('form')).length,
                links: Array.from(document.querySelectorAll('a[href]')).length,
                inputs: Array.from(document.querySelectorAll('input,textarea,select')).length,
            })""")

        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="inspect",
            status="completed",
            summary=f"Inspected ({strategy}): {title}",
            extracted_content=extracted,
            receipts=[
                Receipt(
                    action="inspect",
                    timestamp=utc_now_iso(),
                    success=True,
                    details={"url": url, "title": title, "strategy": strategy},
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame completion (no screenshot for inspect)
        await maybe_record_after(frame, result)
        
        return result
        
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="inspect",
            status="failed",
            summary="Failed to inspect page",
            error=ErrorDetail(code="INSPECT_ERROR", message=str(e)),
            receipts=[
                Receipt(
                    action="inspect",
                    timestamp=utc_now_iso(),
                    success=False,
                    details={"error": str(e)},
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame failure
        await maybe_record_failure(frame, e)
        
        return result


async def handle_close(req: ExecuteRequest) -> ExecuteResponse:
    """Close browser session."""
    trace_id = str(uuid4())
    adapter_id = "browser.playwright"
    
    # Record frame start
    frame = await maybe_record_before(req)
    
    try:
        closed = await session_manager.close_session(req.session_id)
        
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="execute",
            status="completed",
            summary="Session closed" if closed else "Session not found",
            receipts=[
                Receipt(
                    action="close",
                    timestamp=utc_now_iso(),
                    success=closed,
                    details={"was_active": closed},
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame completion
        await maybe_record_after(frame, result)
        
        return result
        
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id,
            session_id=req.session_id,
            adapter_id=adapter_id,
            family="browser",
            mode="execute",
            status="failed",
            summary="Failed to close session",
            error=ErrorDetail(code="CLOSE_ERROR", message=str(e)),
            receipts=[
                Receipt(
                    action="close",
                    timestamp=utc_now_iso(),
                    success=False,
                    details={"error": str(e)},
                )
            ],
            trace_id=trace_id,
        )
        
        # Record frame failure
        await maybe_record_failure(frame, e)
        
        return result


async def handle_scroll(req: ExecuteRequest) -> ExecuteResponse:
    """Scroll the page."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        page = await session_manager.get_page(req.session_id)
        if not page:
            raise Exception("No active session.")
        direction = req.parameters.get("direction", "down")
        amount = int(req.parameters.get("amount", 3))
        px = amount * 100
        js = {
            "down": f"window.scrollBy(0, {px})",
            "up": f"window.scrollBy(0, -{px})",
            "right": f"window.scrollBy({px}, 0)",
            "left": f"window.scrollBy(-{px}, 0)",
        }.get(direction, f"window.scrollBy(0, {px})")
        await page.evaluate(js)
        scroll_y = await page.evaluate("window.scrollY")
        scroll_x = await page.evaluate("window.scrollX")
        after_ss = None
        try:
            after_ss = await page.screenshot(type="png")
        except Exception:
            pass
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Scrolled {direction} {amount} steps",
            extracted_content={"position": {"x": scroll_x, "y": scroll_y}},
            receipts=[Receipt(action="scroll", timestamp=utc_now_iso(), success=True,
                              details={"direction": direction, "amount": amount, "scroll_y": scroll_y})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result, after_ss)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary="Scroll failed",
            error=ErrorDetail(code="SCROLL_ERROR", message=str(e)),
            receipts=[Receipt(action="scroll", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_key(req: ExecuteRequest) -> ExecuteResponse:
    """Press keyboard keys."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        page = await session_manager.get_page(req.session_id)
        if not page:
            raise Exception("No active session.")
        keys = req.parameters.get("keys") or req.target or "Enter"
        await page.keyboard.press(keys)
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Pressed {keys}",
            extracted_content={"keys": keys},
            receipts=[Receipt(action="key", timestamp=utc_now_iso(), success=True,
                              details={"keys": keys})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"Key press failed: {e}",
            error=ErrorDetail(code="KEY_ERROR", message=str(e)),
            receipts=[Receipt(action="key", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_type(req: ExecuteRequest) -> ExecuteResponse:
    """Type text into the focused element (character by character)."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        page = await session_manager.get_page(req.session_id)
        if not page:
            raise Exception("No active session.")
        text = req.parameters.get("text", "") or req.target or ""
        await page.keyboard.type(text)
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Typed {len(text)} chars",
            extracted_content={"chars_typed": len(text)},
            receipts=[Receipt(action="type", timestamp=utc_now_iso(), success=True,
                              details={"chars": len(text)})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary="Type failed",
            error=ErrorDetail(code="TYPE_ERROR", message=str(e)),
            receipts=[Receipt(action="type", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_double_click(req: ExecuteRequest) -> ExecuteResponse:
    """Double click at selector or coordinates."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        page = await session_manager.get_page(req.session_id)
        x = req.parameters.get("x")
        y = req.parameters.get("y")
        if page:
            if x is not None and y is not None:
                await page.mouse.dblclick(float(x), float(y))
            elif req.target:
                await page.dblclick(req.target)
            else:
                raise Exception("target or (x,y) required for double_click")
        else:
            # Fallback: pyautogui
            try:
                import pyautogui as _pag
                if x is not None and y is not None:
                    _pag.doubleClick(float(x), float(y))
                else:
                    raise Exception("No active session and no coordinates for double_click")
            except ImportError:
                raise Exception("No active session. Use 'goto' first or provide coordinates with pyautogui installed.")
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Double clicked {req.target or f'({x},{y})'}",
            receipts=[Receipt(action="double_click", timestamp=utc_now_iso(), success=True,
                              details={"target": req.target, "x": x, "y": y})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"Double click failed: {e}",
            error=ErrorDetail(code="DOUBLE_CLICK_ERROR", message=str(e)),
            receipts=[Receipt(action="double_click", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_right_click(req: ExecuteRequest) -> ExecuteResponse:
    """Right click at selector or coordinates."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        page = await session_manager.get_page(req.session_id)
        x = req.parameters.get("x")
        y = req.parameters.get("y")
        if page:
            if x is not None and y is not None:
                await page.mouse.click(float(x), float(y), button="right")
            elif req.target:
                await page.click(req.target, button="right")
            else:
                raise Exception("target or (x,y) required for right_click")
        else:
            try:
                import pyautogui as _pag
                if x is not None and y is not None:
                    _pag.rightClick(float(x), float(y))
                else:
                    raise Exception("No active session and no coordinates for right_click")
            except ImportError:
                raise Exception("No active session. Use 'goto' first or provide coordinates with pyautogui installed.")
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Right clicked {req.target or f'({x},{y})'}",
            receipts=[Receipt(action="right_click", timestamp=utc_now_iso(), success=True,
                              details={"target": req.target, "x": x, "y": y})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"Right click failed: {e}",
            error=ErrorDetail(code="RIGHT_CLICK_ERROR", message=str(e)),
            receipts=[Receipt(action="right_click", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_drag(req: ExecuteRequest) -> ExecuteResponse:
    """Drag from one coordinate to another."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        from_x = float(req.parameters.get("from_x", 0))
        from_y = float(req.parameters.get("from_y", 0))
        to_x = float(req.parameters.get("to_x", 0))
        to_y = float(req.parameters.get("to_y", 0))
        duration_ms = int(req.parameters.get("duration_ms", 500))
        page = await session_manager.get_page(req.session_id)
        if page:
            steps = max(10, duration_ms // 16)
            await page.mouse.move(from_x, from_y)
            await page.mouse.down()
            await page.mouse.move(to_x, to_y, steps=steps)
            await page.mouse.up()
        else:
            try:
                import pyautogui as _pag
                _pag.moveTo(from_x, from_y)
                _pag.dragTo(to_x, to_y, duration=duration_ms / 1000.0)
            except ImportError:
                raise Exception("No active session and pyautogui not installed.")
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Dragged ({from_x},{from_y}) → ({to_x},{to_y})",
            receipts=[Receipt(action="drag", timestamp=utc_now_iso(), success=True,
                              details={"from_x": from_x, "from_y": from_y, "to_x": to_x, "to_y": to_y})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"Drag failed: {e}",
            error=ErrorDetail(code="DRAG_ERROR", message=str(e)),
            receipts=[Receipt(action="drag", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_key_combo(req: ExecuteRequest) -> ExecuteResponse:
    """Press a key combination like cmd+s, ctrl+shift+t."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        combo = req.target or req.parameters.get("keys", "")
        if not combo:
            raise Exception("target (key combo string) is required for key_combo")
        page = await session_manager.get_page(req.session_id)
        if page:
            # Playwright uses + separated keys; convert cmd→Meta, ctrl→Control
            pw_combo = (combo
                        .replace("cmd", "Meta")
                        .replace("ctrl", "Control")
                        .replace("alt", "Alt")
                        .replace("shift", "Shift"))
            await page.keyboard.press(pw_combo)
        else:
            try:
                import pyautogui as _pag
                parts = [p.strip() for p in combo.split("+")]
                _pag.hotkey(*parts)
            except ImportError:
                # AppleScript fallback
                import subprocess
                script = f'tell application "System Events" to keystroke "{combo}"'
                subprocess.run(["osascript", "-e", script], timeout=5)
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Pressed combo {combo!r}",
            extracted_content={"combo": combo},
            receipts=[Receipt(action="key_combo", timestamp=utc_now_iso(), success=True,
                              details={"combo": combo})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"Key combo failed: {e}",
            error=ErrorDetail(code="KEY_COMBO_ERROR", message=str(e)),
            receipts=[Receipt(action="key_combo", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_set_value(req: ExecuteRequest) -> ExecuteResponse:
    """Set the value of an element by ref or selector."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        value = req.parameters.get("value", req.parameters.get("text", ""))
        selector = req.target or ""
        page = await session_manager.get_page(req.session_id)
        if page and selector:
            await page.fill(selector, str(value))
        else:
            try:
                from core.accessibility_inspector import AccessibilityInspector
                _insp = AccessibilityInspector()
                await _insp.set_value(selector, value)
            except Exception:
                raise Exception(f"Cannot set_value: no page session and accessibility inspector unavailable")
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.accessibility", family="desktop", mode="execute",
            status="completed", summary=f"Set value of {selector!r}",
            receipts=[Receipt(action="set_value", timestamp=utc_now_iso(), success=True,
                              details={"selector": selector, "value": value})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.accessibility", family="desktop", mode="execute",
            status="failed", summary=f"set_value failed: {e}",
            error=ErrorDetail(code="SET_VALUE_ERROR", message=str(e)),
            receipts=[Receipt(action="set_value", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_toggle(req: ExecuteRequest) -> ExecuteResponse:
    """Toggle a checkbox or switch element."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        selector = req.target or ""
        page = await session_manager.get_page(req.session_id)
        if page and selector:
            await page.click(selector)
        else:
            try:
                from core.accessibility_inspector import AccessibilityInspector
                _insp = AccessibilityInspector()
                await _insp.toggle(selector)
            except Exception:
                raise Exception(f"Cannot toggle: no page session and accessibility inspector unavailable")
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.accessibility", family="desktop", mode="execute",
            status="completed", summary=f"Toggled {selector!r}",
            receipts=[Receipt(action="toggle", timestamp=utc_now_iso(), success=True,
                              details={"selector": selector})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.accessibility", family="desktop", mode="execute",
            status="failed", summary=f"toggle failed: {e}",
            error=ErrorDetail(code="TOGGLE_ERROR", message=str(e)),
            receipts=[Receipt(action="toggle", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_expand(req: ExecuteRequest) -> ExecuteResponse:
    """Expand a collapsible element (tree node, dropdown)."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        selector = req.target or ""
        page = await session_manager.get_page(req.session_id)
        if page and selector:
            await page.click(selector)
        else:
            try:
                from core.accessibility_inspector import AccessibilityInspector
                _insp = AccessibilityInspector()
                await _insp.expand(selector)
            except Exception:
                raise Exception(f"Cannot expand: no page session and accessibility inspector unavailable")
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.accessibility", family="desktop", mode="execute",
            status="completed", summary=f"Expanded {selector!r}",
            receipts=[Receipt(action="expand", timestamp=utc_now_iso(), success=True,
                              details={"selector": selector})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.accessibility", family="desktop", mode="execute",
            status="failed", summary=f"expand failed: {e}",
            error=ErrorDetail(code="EXPAND_ERROR", message=str(e)),
            receipts=[Receipt(action="expand", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_collapse(req: ExecuteRequest) -> ExecuteResponse:
    """Collapse an expanded element."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        selector = req.target or ""
        page = await session_manager.get_page(req.session_id)
        if page and selector:
            await page.click(selector)
        else:
            try:
                from core.accessibility_inspector import AccessibilityInspector
                _insp = AccessibilityInspector()
                await _insp.collapse(selector)
            except Exception:
                raise Exception(f"Cannot collapse: no page session and accessibility inspector unavailable")
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.accessibility", family="desktop", mode="execute",
            status="completed", summary=f"Collapsed {selector!r}",
            receipts=[Receipt(action="collapse", timestamp=utc_now_iso(), success=True,
                              details={"selector": selector})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.accessibility", family="desktop", mode="execute",
            status="failed", summary=f"collapse failed: {e}",
            error=ErrorDetail(code="COLLAPSE_ERROR", message=str(e)),
            receipts=[Receipt(action="collapse", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_hover(req: ExecuteRequest) -> ExecuteResponse:
    """Hover over element or coordinates."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        x = req.parameters.get("x")
        y = req.parameters.get("y")
        selector = req.target or ""
        page = await session_manager.get_page(req.session_id)
        if page:
            if x is not None and y is not None:
                await page.mouse.move(float(x), float(y))
            elif selector:
                await page.hover(selector)
            else:
                raise Exception("target or (x,y) required for hover")
        else:
            try:
                import pyautogui as _pag
                if x is not None and y is not None:
                    _pag.moveTo(float(x), float(y))
                else:
                    raise Exception("No active session and no coordinates for hover")
            except ImportError:
                raise Exception("No active session. Use 'goto' first or provide coordinates with pyautogui installed.")
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Hovered over {selector or f'({x},{y})'}",
            receipts=[Receipt(action="hover", timestamp=utc_now_iso(), success=True,
                              details={"selector": selector, "x": x, "y": y})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"hover failed: {e}",
            error=ErrorDetail(code="HOVER_ERROR", message=str(e)),
            receipts=[Receipt(action="hover", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_triple_click(req: ExecuteRequest) -> ExecuteResponse:
    """Triple click (select all text in field)."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        x = req.parameters.get("x")
        y = req.parameters.get("y")
        selector = req.target or ""
        page = await session_manager.get_page(req.session_id)
        if page:
            if x is not None and y is not None:
                await page.mouse.click(float(x), float(y), click_count=3)
            elif selector:
                await page.click(selector, click_count=3)
            else:
                raise Exception("target or (x,y) required for triple_click")
        else:
            try:
                import pyautogui as _pag
                if x is not None and y is not None:
                    _pag.tripleClick(float(x), float(y))
                else:
                    raise Exception("No active session and no coordinates for triple_click")
            except ImportError:
                raise Exception("No active session. Use 'goto' first or provide coordinates with pyautogui installed.")
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Triple clicked {selector or f'({x},{y})'}",
            receipts=[Receipt(action="triple_click", timestamp=utc_now_iso(), success=True,
                              details={"selector": selector, "x": x, "y": y})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"triple_click failed: {e}",
            error=ErrorDetail(code="TRIPLE_CLICK_ERROR", message=str(e)),
            receipts=[Receipt(action="triple_click", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_get_clipboard(req: ExecuteRequest) -> ExecuteResponse:
    """Get current clipboard contents."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        content: str = ""
        # Try pyperclip first (cross-platform)
        try:
            import pyperclip as _pyclip
            content = _pyclip.paste() or ""
        except ImportError:
            # AppleScript fallback on macOS
            import subprocess
            result_proc = subprocess.run(
                ["osascript", "-e", "get the clipboard"],
                capture_output=True, text=True, timeout=5
            )
            content = result_proc.stdout.strip()
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.system", family="desktop", mode="inspect",
            status="completed", summary=f"Clipboard contents ({len(content)} chars)",
            extracted_content={"clipboard": content, "length": len(content)},
            receipts=[Receipt(action="get_clipboard", timestamp=utc_now_iso(), success=True,
                              details={"length": len(content)})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.system", family="desktop", mode="inspect",
            status="failed", summary=f"get_clipboard failed: {e}",
            error=ErrorDetail(code="CLIPBOARD_ERROR", message=str(e)),
            receipts=[Receipt(action="get_clipboard", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_set_clipboard(req: ExecuteRequest) -> ExecuteResponse:
    """Set clipboard contents. target contains the text."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        text = req.target or req.parameters.get("text", "")
        try:
            import pyperclip as _pyclip
            _pyclip.copy(text)
        except ImportError:
            import subprocess
            escaped = text.replace('"', '\\"')
            subprocess.run(
                ["osascript", "-e", f'set the clipboard to "{escaped}"'],
                timeout=5
            )
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.system", family="desktop", mode="execute",
            status="completed", summary=f"Set clipboard ({len(text)} chars)",
            receipts=[Receipt(action="set_clipboard", timestamp=utc_now_iso(), success=True,
                              details={"length": len(text)})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.system", family="desktop", mode="execute",
            status="failed", summary=f"set_clipboard failed: {e}",
            error=ErrorDetail(code="CLIPBOARD_ERROR", message=str(e)),
            receipts=[Receipt(action="set_clipboard", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_find_elements(req: ExecuteRequest) -> ExecuteResponse:
    """Find elements matching query in the AX tree. target is the query."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        query = req.target or req.parameters.get("query", "")
        elements: List[Dict] = []
        try:
            from core.accessibility_inspector import AccessibilityInspector
            _insp = AccessibilityInspector()
            if await _insp.is_available():
                elements = await _insp.find_elements(query)
        except Exception:
            pass
        # Browser fallback — CSS/text selector search
        if not elements:
            page = await session_manager.get_page(req.session_id)
            if page and query:
                try:
                    els = await page.query_selector_all(query)
                    for el in els[:20]:
                        bbox = await el.bounding_box()
                        text = await el.inner_text()
                        elements.append({"text": text[:100], "bounds": bbox})
                except Exception:
                    pass
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.accessibility", family="desktop", mode="inspect",
            status="completed", summary=f"Found {len(elements)} elements matching {query!r}",
            extracted_content={"elements": elements, "count": len(elements), "query": query},
            receipts=[Receipt(action="find_elements", timestamp=utc_now_iso(), success=True,
                              details={"query": query, "count": len(elements)})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.accessibility", family="desktop", mode="inspect",
            status="failed", summary=f"find_elements failed: {e}",
            error=ErrorDetail(code="FIND_ELEMENTS_ERROR", message=str(e)),
            receipts=[Receipt(action="find_elements", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_ax_snapshot(req: ExecuteRequest) -> ExecuteResponse:
    """Get full accessibility tree snapshot for the current app."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        skeleton = req.parameters.get("skeleton", False)
        window_id = req.parameters.get("window_id")
        depth = req.parameters.get("depth", -1)
        ax_data: Dict = {}
        try:
            from core.accessibility_inspector import AccessibilityInspector
            _insp = AccessibilityInspector()
            if await _insp.is_available():
                ax_root = await _insp.snapshot(skeleton=skeleton, window_id=window_id, depth=depth)
                if ax_root:
                    ax_data = ax_root.to_dict(compact=skeleton)
        except Exception as _ax_err:
            ax_data = {"error": f"accessibility inspector unavailable: {_ax_err}"}
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.accessibility", family="desktop", mode="inspect",
            status="completed", summary="AX snapshot captured",
            extracted_content={"ax_tree": ax_data, "skeleton": skeleton, "window_id": window_id},
            receipts=[Receipt(action="ax_snapshot", timestamp=utc_now_iso(), success=True,
                              details={"skeleton": skeleton, "window_id": window_id})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.accessibility", family="desktop", mode="inspect",
            status="failed", summary=f"ax_snapshot failed: {e}",
            error=ErrorDetail(code="AX_SNAPSHOT_ERROR", message=str(e)),
            receipts=[Receipt(action="ax_snapshot", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


def _action_coordinate(req: ExecuteRequest) -> tuple[float | None, float | None]:
    """Extract [x, y] from the SDK's `coordinate` array or loose x/y parameters."""
    coord = req.parameters.get("coordinate")
    if isinstance(coord, (list, tuple)) and len(coord) >= 2:
        return float(coord[0]), float(coord[1])
    x, y = req.parameters.get("x"), req.parameters.get("y")
    if x is not None and y is not None:
        return float(x), float(y)
    return None, None


def _action_duration_seconds(req: ExecuteRequest, default: float = 1.0) -> float:
    """Duration in seconds for hold_key/wait, from `duration` or `duration_ms`."""
    duration = req.parameters.get("duration")
    if duration is not None:
        return max(0.0, float(duration))
    duration_ms = req.parameters.get("duration_ms")
    if duration_ms is not None:
        return max(0.0, float(duration_ms) / 1000.0)
    return default


async def handle_mouse_move(req: ExecuteRequest) -> ExecuteResponse:
    """Move the mouse cursor to an absolute [x, y] pixel coordinate."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        x, y = _action_coordinate(req)
        if x is None or y is None:
            raise Exception("coordinate [x, y] is required for mouse_move")
        page = await session_manager.get_page(req.session_id)
        if page:
            await page.mouse.move(x, y)
        else:
            try:
                import pyautogui as _pag
                _pag.moveTo(x, y)
            except ImportError:
                raise Exception("mouse_move requires an active browser session or pyautogui for desktop control")
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Moved mouse to ({x}, {y})",
            extracted_content={"x": x, "y": y},
            receipts=[Receipt(action="mouse_move", timestamp=utc_now_iso(), success=True,
                              details={"x": x, "y": y})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"mouse_move failed: {e}",
            error=ErrorDetail(code="MOUSE_MOVE_ERROR", message=str(e)),
            receipts=[Receipt(action="mouse_move", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def _handle_mouse_click_variant(req: ExecuteRequest, action: str, button: str) -> ExecuteResponse:
    """Shared left/middle click handler; coordinate optional (clicks at current position)."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        x, y = _action_coordinate(req)
        page = await session_manager.get_page(req.session_id)
        if page:
            if x is not None and y is not None:
                await page.mouse.click(x, y, button=button)
            else:
                # Playwright has no real cursor in the browser context — click
                # at the current virtual mouse position.
                await page.mouse.down(button=button)
                await page.mouse.up(button=button)
        else:
            try:
                import pyautogui as _pag
                if x is not None and y is not None:
                    _pag.click(x, y, button=button)
                else:
                    _pag.click(button=button)
            except ImportError:
                raise Exception(f"{action} requires an active browser session or pyautogui for desktop control")
        where = f"({x}, {y})" if x is not None and y is not None else "current position"
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"{button} clicked at {where}",
            extracted_content={"button": button, "x": x, "y": y},
            receipts=[Receipt(action=action, timestamp=utc_now_iso(), success=True,
                              details={"button": button, "x": x, "y": y})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"{action} failed: {e}",
            error=ErrorDetail(code=f"{action.upper()}_ERROR", message=str(e)),
            receipts=[Receipt(action=action, timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_left_click(req: ExecuteRequest) -> ExecuteResponse:
    """Left click at [x, y] or at the current cursor position."""
    return await _handle_mouse_click_variant(req, "left_click", "left")


async def handle_middle_click(req: ExecuteRequest) -> ExecuteResponse:
    """Middle click at [x, y] or at the current cursor position."""
    return await _handle_mouse_click_variant(req, "middle_click", "middle")


async def handle_left_click_drag(req: ExecuteRequest) -> ExecuteResponse:
    """Drag from the start coordinate to a target path or [to_x, to_y]."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        x, y = _action_coordinate(req)
        if x is None or y is None:
            raise Exception("coordinate [x, y] (drag start) is required for left_click_drag")
        path = req.parameters.get("path")
        to = req.parameters.get("to")
        if isinstance(path, (list, tuple)) and path:
            end = path[-1]
            to_x, to_y = float(end[0]), float(end[1])
        elif isinstance(to, (list, tuple)) and len(to) >= 2:
            to_x, to_y = float(to[0]), float(to[1])
        else:
            to_x = req.parameters.get("to_x")
            to_y = req.parameters.get("to_y")
        if to_x is None or to_y is None:
            raise Exception("drag target required: parameters.path, parameters.to [x, y], or to_x/to_y")
        to_x, to_y = float(to_x), float(to_y)
        duration_ms = int(req.parameters.get("duration_ms", 500))
        page = await session_manager.get_page(req.session_id)
        if page:
            steps = max(10, duration_ms // 16)
            await page.mouse.move(x, y)
            await page.mouse.down()
            await page.mouse.move(to_x, to_y, steps=steps)
            await page.mouse.up()
        else:
            try:
                import pyautogui as _pag
                _pag.moveTo(x, y)
                _pag.dragTo(to_x, to_y, duration=duration_ms / 1000.0, button="left")
            except ImportError:
                raise Exception("left_click_drag requires an active browser session or pyautogui for desktop control")
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Dragged ({x}, {y}) → ({to_x}, {to_y})",
            extracted_content={"from_x": x, "from_y": y, "to_x": to_x, "to_y": to_y},
            receipts=[Receipt(action="left_click_drag", timestamp=utc_now_iso(), success=True,
                              details={"from_x": x, "from_y": y, "to_x": to_x, "to_y": to_y})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"left_click_drag failed: {e}",
            error=ErrorDetail(code="LEFT_CLICK_DRAG_ERROR", message=str(e)),
            receipts=[Receipt(action="left_click_drag", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def _handle_mouse_button_state(req: ExecuteRequest, action: str, down: bool) -> ExecuteResponse:
    """Shared left_mouse_down / left_mouse_up handler."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        x, y = _action_coordinate(req)
        page = await session_manager.get_page(req.session_id)
        if page:
            if x is not None and y is not None:
                await page.mouse.move(x, y)
            if down:
                await page.mouse.down()
            else:
                await page.mouse.up()
        else:
            try:
                import pyautogui as _pag
                if x is not None and y is not None:
                    _pag.moveTo(x, y)
                if down:
                    _pag.mouseDown(button="left")
                else:
                    _pag.mouseUp(button="left")
            except ImportError:
                raise Exception(f"{action} requires an active browser session or pyautogui for desktop control")
        state = "pressed" if down else "released"
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Left mouse button {state}",
            extracted_content={"button": "left", "state": state, "x": x, "y": y},
            receipts=[Receipt(action=action, timestamp=utc_now_iso(), success=True,
                              details={"button": "left", "state": state, "x": x, "y": y})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"{action} failed: {e}",
            error=ErrorDetail(code=f"{action.upper()}_ERROR", message=str(e)),
            receipts=[Receipt(action=action, timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_left_mouse_down(req: ExecuteRequest) -> ExecuteResponse:
    """Press and hold the left mouse button (at [x, y] if given)."""
    return await _handle_mouse_button_state(req, "left_mouse_down", down=True)


async def handle_left_mouse_up(req: ExecuteRequest) -> ExecuteResponse:
    """Release the left mouse button (at [x, y] if given)."""
    return await _handle_mouse_button_state(req, "left_mouse_up", down=False)


async def handle_cursor_position(req: ExecuteRequest) -> ExecuteResponse:
    """Return the current OS cursor position. Desktop-only (pyautogui)."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        try:
            import pyautogui as _pag
        except ImportError:
            raise Exception(
                "cursor_position is a desktop action and requires pyautogui; "
                "it is not supported on this platform/backend"
            )
        pos = _pag.position()
        x, y = float(pos[0]), float(pos[1])
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.pyautogui", family="desktop", mode="inspect",
            status="completed", summary=f"Cursor at ({x}, {y})",
            extracted_content={"x": x, "y": y},
            receipts=[Receipt(action="cursor_position", timestamp=utc_now_iso(), success=True,
                              details={"x": x, "y": y})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="desktop.pyautogui", family="desktop", mode="inspect",
            status="failed", summary=f"cursor_position failed: {e}",
            error=ErrorDetail(code="CURSOR_POSITION_ERROR", message=str(e)),
            receipts=[Receipt(action="cursor_position", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_hold_key(req: ExecuteRequest) -> ExecuteResponse:
    """Hold a key down for `duration` seconds (default 1.0)."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        key = req.parameters.get("key") or req.target or req.text
        if not key:
            raise Exception("key is required for hold_key")
        duration = _action_duration_seconds(req, default=1.0)
        page = await session_manager.get_page(req.session_id)
        if page:
            await page.keyboard.down(key)
            await asyncio.sleep(duration)
            await page.keyboard.up(key)
        else:
            try:
                import pyautogui as _pag
                _pag.keyDown(key)
                await asyncio.sleep(duration)
                _pag.keyUp(key)
            except ImportError:
                raise Exception("hold_key requires an active browser session or pyautogui for desktop control")
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Held {key} for {duration}s",
            extracted_content={"key": key, "duration": duration},
            receipts=[Receipt(action="hold_key", timestamp=utc_now_iso(), success=True,
                              details={"key": key, "duration": duration})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"hold_key failed: {e}",
            error=ErrorDetail(code="HOLD_KEY_ERROR", message=str(e)),
            receipts=[Receipt(action="hold_key", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_wait(req: ExecuteRequest) -> ExecuteResponse:
    """Wait for `duration` seconds (default 1.0). Always available."""
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    try:
        duration = _action_duration_seconds(req, default=1.0)
        await asyncio.sleep(duration)
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="completed", summary=f"Waited {duration}s",
            extracted_content={"duration": duration},
            receipts=[Receipt(action="wait", timestamp=utc_now_iso(), success=True,
                              details={"duration": duration})],
            trace_id=trace_id,
        )
        await maybe_record_after(frame, result)
        return result
    except Exception as e:
        result = ExecuteResponse(
            run_id=req.run_id, session_id=req.session_id,
            adapter_id="browser.playwright", family="browser", mode="execute",
            status="failed", summary=f"wait failed: {e}",
            error=ErrorDetail(code="WAIT_ERROR", message=str(e)),
            receipts=[Receipt(action="wait", timestamp=utc_now_iso(), success=False,
                              details={"error": str(e)})],
            trace_id=trace_id,
        )
        await maybe_record_failure(frame, e)
        return result


async def handle_zoom(req: ExecuteRequest) -> ExecuteResponse:
    """computer_20251124 'zoom' — no platform gesture backend on this gateway.

    Fails explicitly rather than silently no-op'ing; models can retry with
    scroll + cursor_position which are supported.
    """
    trace_id = str(uuid4())
    frame = await maybe_record_before(req)
    message = (
        "zoom (computer_20251124) is not supported by this gateway: no platform "
        "magnification-gesture backend is registered. Use scroll at a coordinate "
        "or cursor_position + left_click_drag instead."
    )
    result = ExecuteResponse(
        run_id=req.run_id, session_id=req.session_id,
        adapter_id="browser.stub", family="browser", mode="execute",
        status="failed", summary="zoom is not supported on this platform",
        error=ErrorDetail(code="UNSUPPORTED_PLATFORM", message=message),
        receipts=[Receipt(action="zoom", timestamp=utc_now_iso(), success=False,
                          details={"error": message})],
        trace_id=trace_id,
    )
    await maybe_record_failure(frame, RuntimeError(message))
    return result


async def handle_stub(req: ExecuteRequest) -> ExecuteResponse:
    """Fallback for unknown/unimplemented actions — always fails explicitly."""
    trace_id = str(uuid4())
    return ExecuteResponse(
        run_id=req.run_id,
        session_id=req.session_id,
        adapter_id="browser.stub",
        family="browser",
        mode="execute",
        status="failed",
        summary=f"Unknown action: {req.action!r}",
        error=ErrorDetail(
            code="UNKNOWN_ACTION",
            message=(
                f"Action '{req.action}' is not implemented. Supported: "
                "goto, screenshot, click, double_click, right_click, fill, type, extract, "
                "inspect, close, scroll, key, key_combo, drag, hover, triple_click, "
                "set_value, toggle, expand, collapse, get_clipboard, set_clipboard, "
                "find_elements, ax_snapshot, mouse_move, left_click, left_click_drag, "
                "middle_click, left_mouse_down, left_mouse_up, cursor_position, "
                "hold_key, wait, zoom (explicit unsupported-platform error)"
            ),
        ),
        receipts=[
            Receipt(
                action=req.action,
                timestamp=utc_now_iso(),
                success=False,
                details={"error": f"Action '{req.action}' not implemented"},
            )
        ],
        trace_id=trace_id,
    )


# ============================================================================
# API Endpoints
# ============================================================================

@app.get("/health")
async def health() -> dict[str, Any]:
    """Health check with session stats."""
    stats = await session_manager.get_session_stats()
    return {
        "status": "ok",
        "version": "0.3.0",
        "playwright": "enabled",
        "ax_permission": _ax_permission_granted,
        "sessions": {
            "active": stats["active_sessions"],
            "max": stats["max_sessions"],
        },
    }


@app.post("/v1/execute", response_model=ExecuteResponse)
async def execute(req: ExecuteRequest) -> ExecuteResponse:
    """Execute a browser automation action with persistent sessions."""
    validate_request(req)
    
    # Route to appropriate handler
    handlers = {
        "goto": handle_goto,
        "navigate": handle_goto,
        "screenshot": handle_screenshot,
        "click": handle_click,
        "double_click": handle_double_click,
        "right_click": handle_right_click,
        "triple_click": handle_triple_click,
        "hover": handle_hover,
        "fill": handle_fill,
        "type": handle_type,
        "extract": handle_extract,
        "inspect": handle_inspect,
        "close": handle_close,
        "scroll": handle_scroll,
        "key": handle_key,
        "key_combo": handle_key_combo,
        "drag": handle_drag,
        "set_value": handle_set_value,
        "toggle": handle_toggle,
        "expand": handle_expand,
        "collapse": handle_collapse,
        "get_clipboard": handle_get_clipboard,
        "set_clipboard": handle_set_clipboard,
        "find_elements": handle_find_elements,
        "ax_snapshot": handle_ax_snapshot,
        "mouse_move": handle_mouse_move,
        "left_click": handle_left_click,
        "left_click_drag": handle_left_click_drag,
        "middle_click": handle_middle_click,
        "left_mouse_down": handle_left_mouse_down,
        "left_mouse_up": handle_left_mouse_up,
        "cursor_position": handle_cursor_position,
        "hold_key": handle_hold_key,
        "wait": handle_wait,
        "zoom": handle_zoom,
    }

    action = req.action
    # "execute" is a meta-action: re-dispatch on the sub-action inside parameters
    if action == "execute":
        sub = req.parameters.get("action") or req.parameters.get("action_type", "")
        action = sub or "screenshot"

    handler = handlers.get(action, handle_stub)
    return await handler(req)


class ExtractRequest(BaseModel):
    session_id: str
    format: str = "text"
    selector: str | None = None
    run_id: str = Field(default_factory=lambda: str(uuid4()))


@app.post("/v1/extract", response_model=ExecuteResponse)
async def extract_endpoint(req: ExtractRequest) -> ExecuteResponse:
    """Dedicated extract endpoint — cleaner API than /v1/execute?action=extract."""
    execute_req = ExecuteRequest(
        action="extract",
        session_id=req.session_id,
        run_id=req.run_id,
        target=req.selector,
        parameters={"format": req.format, "mode": req.format},
    )
    return await handle_extract(execute_req)


@app.get("/v1/routes")
async def get_routes() -> dict[str, Any]:
    """Self-documenting API route registry."""
    try:
        import sys as _sys, os as _os
        _sys.path.insert(0, _os.path.dirname(__file__))
        from routes_registry import ROUTE_REGISTRY
        return {"routes": [r.to_dict() for r in ROUTE_REGISTRY], "count": len(ROUTE_REGISTRY)}
    except Exception as e:
        return {"routes": [], "count": 0, "error": str(e)}


@app.get("/v1/apps")
async def list_apps_endpoint() -> dict[str, Any]:
    """List running applications."""
    try:
        import sys as _sys, os as _os
        _sys.path.insert(0, _os.path.join(_os.path.dirname(__file__), ".."))
        from core.accessibility_inspector import AccessibilityInspector
        inspector = AccessibilityInspector()
        apps = await inspector.list_apps()
        return {"apps": [a.__dict__ for a in apps], "count": len(apps)}
    except Exception as e:
        return {"apps": [], "count": 0, "error": str(e)}


@app.get("/v1/windows")
async def list_windows_endpoint(app_name: str | None = None) -> dict[str, Any]:
    """List open windows, optionally filtered by app name."""
    try:
        import sys as _sys, os as _os
        _sys.path.insert(0, _os.path.join(_os.path.dirname(__file__), ".."))
        from core.accessibility_inspector import AccessibilityInspector
        inspector = AccessibilityInspector()
        windows = await inspector.list_windows(app_name=app_name)
        return {"windows": [w.__dict__ for w in windows], "count": len(windows)}
    except Exception as e:
        return {"windows": [], "count": 0, "error": str(e)}


@app.get("/v1/window-state")
async def get_window_state(window_id: int | None = None, skeleton: bool = False) -> dict[str, Any]:
    """Get AX tree + screenshot + coordinate contract for a window."""
    try:
        import sys as _sys, os as _os
        _sys.path.insert(0, _os.path.join(_os.path.dirname(__file__), ".."))
        from core.accessibility_inspector import AccessibilityInspector
        inspector = AccessibilityInspector()
        node = await inspector.snapshot(skeleton=skeleton, window_id=window_id)
        contract = await inspector.get_coordinate_contract(window_id=window_id)
        return {
            "window_id": window_id,
            "ax_tree": node.to_dict(compact=True) if node else None,
            "coordinate_contract": contract.to_dict() if contract else None,
        }
    except Exception as e:
        return {"window_id": window_id, "error": str(e)}


@app.get("/v1/notifications")
async def list_notifications_endpoint() -> dict[str, Any]:
    """List notification center items."""
    try:
        import sys as _sys, os as _os
        _sys.path.insert(0, _os.path.join(_os.path.dirname(__file__), ".."))
        from core.accessibility_inspector import AccessibilityInspector
        inspector = AccessibilityInspector()
        notifs = await inspector.list_notifications()
        return {"notifications": [n.__dict__ for n in notifs], "count": len(notifs)}
    except Exception as e:
        return {"notifications": [], "count": 0, "error": str(e)}


@app.post("/v1/notifications/{notification_id}/dismiss")
async def dismiss_notification_endpoint(notification_id: str) -> dict[str, Any]:
    """Dismiss a notification."""
    try:
        import sys as _sys, os as _os
        _sys.path.insert(0, _os.path.join(_os.path.dirname(__file__), ".."))
        from core.accessibility_inspector import AccessibilityInspector
        inspector = AccessibilityInspector()
        ok = await inspector.dismiss_notification(notification_id)
        return {"success": ok}
    except Exception as e:
        return {"success": False, "error": str(e)}


@app.post("/v1/notifications/{notification_id}/action")
async def notification_action_endpoint(notification_id: str, body: NotificationActionRequest) -> dict[str, Any]:
    """Perform a notification action."""
    try:
        import sys as _sys, os as _os
        _sys.path.insert(0, _os.path.join(_os.path.dirname(__file__), ".."))
        from core.accessibility_inspector import AccessibilityInspector
        inspector = AccessibilityInspector()
        ok = await inspector.perform_notification_action(notification_id, body.action)
        return {"success": ok}
    except Exception as e:
        return {"success": False, "error": str(e)}


@app.post("/v1/apps/launch")
async def launch_app_endpoint(body: AppLaunchRequest) -> dict[str, Any]:
    """Launch an application."""
    try:
        import subprocess
        cmd = ["open", "-a", body.name]
        if body.bundle_id:
            cmd = ["open", "-b", body.bundle_id]
        subprocess.Popen(cmd)
        return {"success": True, "name": body.name}
    except Exception as e:
        return {"success": False, "error": str(e)}


@app.post("/v1/apps/close")
async def close_app_endpoint(body: AppCloseRequest) -> dict[str, Any]:
    """Quit a running application."""
    try:
        import subprocess
        script = f'tell application "{body.name}" to quit'
        result = subprocess.run(["osascript", "-e", script], capture_output=True, timeout=5)
        return {"success": result.returncode == 0}
    except Exception as e:
        return {"success": False, "error": str(e)}


@app.post("/v1/windows/focus")
async def focus_window_endpoint(body: WindowFocusRequest) -> dict[str, Any]:
    """Bring a window to the foreground."""
    try:
        import subprocess
        target = body.app_name or body.title or ""
        if target:
            script = f'tell application "{target}" to activate'
            result = subprocess.run(["osascript", "-e", script], capture_output=True, timeout=5)
            return {"success": result.returncode == 0}
        return {"success": False, "error": "No target specified"}
    except Exception as e:
        return {"success": False, "error": str(e)}


@app.post("/v1/windows/resize")
async def resize_window_endpoint(body: WindowResizeRequest) -> dict[str, Any]:
    """Resize a window."""
    try:
        import sys as _sys, os as _os
        _sys.path.insert(0, _os.path.join(_os.path.dirname(__file__), ".."))
        from core.window_motion import WindowMotionEngine, Frame
        engine = WindowMotionEngine()
        if body.x is not None and body.y is not None and body.width is not None and body.height is not None:
            plan = await engine.plan_set_frame(body.window_id, Frame(body.x, body.y, body.width, body.height))
        elif body.edge and body.delta is not None:
            from core.window_motion import ResizeEdge
            edge = ResizeEdge(body.edge)
            plan = await engine.plan_resize(body.window_id, edge, body.delta)
        else:
            return {"success": False, "error": "Provide x/y/width/height or edge/delta"}
        result = await engine.execute(plan)
        return {"success": result.success, "notes": result.notes}
    except Exception as e:
        return {"success": False, "error": str(e)}


@app.post("/v1/windows/drag")
async def drag_window_endpoint(body: WindowDragRequest) -> dict[str, Any]:
    """Move a window by drag."""
    try:
        import sys as _sys, os as _os
        _sys.path.insert(0, _os.path.join(_os.path.dirname(__file__), ".."))
        from core.window_motion import WindowMotionEngine
        engine = WindowMotionEngine()
        plan = await engine.plan_drag(body.window_id, body.delta_x, body.delta_y)
        result = await engine.execute(plan)
        return {"success": result.success, "notes": result.notes}
    except Exception as e:
        return {"success": False, "error": str(e)}


@app.post("/v1/finalize")
async def finalize_run(run_id: str, build_replay: bool = True) -> dict[str, Any]:
    """
    Finalize a run and generate observability artifacts.
    
    Call this after a sequence of actions to:
    - Save timeline
    - Build replay GIF/video
    - Generate analysis
    """
    result = await finalize_run_observability(run_id, build_replay)
    return result or {"status": "observability_disabled"}


@app.get("/v1/recordings/{run_id}")
async def get_recording(run_id: str) -> dict[str, Any]:
    """Get recording data for a run."""
    from observability_integration import recorder
    
    if recorder is None:
        return {"error": "observability_disabled"}
    
    timeline = await recorder.get_timeline(run_id)
    if timeline is None:
        raise HTTPException(status_code=404, detail="Recording not found")
    
    return {
        "run_id": run_id,
        "session_id": timeline.session_id,
        "total_steps": timeline.total_steps,
        "completed": timeline.completed_steps,
        "failed": timeline.failed_steps,
        "replay_gif": timeline.replay_gif_path,
        "replay_video": timeline.replay_video_path,
    }


@app.get("/v1/receipts/verify/{receipt_id}")
async def verify_receipt(receipt_id: str) -> dict[str, Any]:
    """
    Verify the integrity of a stored receipt by re-computing its SHA-256 hash.

    Returns whether the stored hash matches the re-computed hash so callers can
    detect tampering.
    """
    receipts_dir = Path(os.path.expanduser("~/.allternit/receipts"))
    receipt_path = receipts_dir / f"{receipt_id}.json"

    if not receipt_path.exists():
        raise HTTPException(status_code=404, detail=f"Receipt '{receipt_id}' not found")

    try:
        with open(receipt_path) as f:
            receipt = json.load(f)
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Failed to read receipt: {e}")

    stored_hash: str = receipt.get("integrity_hash", "")

    # Re-compute hash using the same algorithm as ActionReceipt.compute_integrity:
    # SHA-256 over json.dumps({"action": action_data, "result": result_data}, sort_keys=True)
    # The stored receipt doesn't have separate action_data/result_data fields; instead
    # the original hash was computed from caller-supplied dicts passed to emit().
    # We reconstruct equivalent deterministic payloads from the fields that ARE persisted.
    action_data = {
        "action_type": receipt.get("action_type", ""),
        "adapter_id": receipt.get("adapter_id", ""),
        "target": receipt.get("target", ""),
        "run_id": receipt.get("run_id", ""),
        "session_id": receipt.get("session_id", ""),
        "policy_decision_id": receipt.get("policy_decision_id"),
        "duration_ms": receipt.get("duration_ms", 0),
    }
    result_data = {
        "status": receipt.get("status", ""),
        "error": receipt.get("error"),
        "artifact_refs": receipt.get("artifact_refs", []),
        "before_evidence": receipt.get("before_evidence"),
        "after_evidence": receipt.get("after_evidence"),
    }
    payload = json.dumps(
        {"action": action_data, "result": result_data},
        sort_keys=True,
        default=str,
    )
    computed_hash = hashlib.sha256(payload.encode()).hexdigest()

    valid = stored_hash == computed_hash
    return {
        "receipt_id": receipt_id,
        "valid": valid,
        "stored_hash": stored_hash,
        "computed_hash": computed_hash,
        "tampered": not valid,
        "timestamp": receipt.get("timestamp", ""),
    }


@app.on_event("startup")
async def startup_event():
    """Initialize session manager, observability, and adapter waterfall on startup."""
    global _ax_permission_granted
    await session_manager.initialize()
    init_observability()
    await _register_startup_adapters()
    try:
        from run_persistence import RunPersistence
        from computer_use_router import _run_store

        _run_store.attach_persistence(RunPersistence())
        logger.info("Run persistence attached (%s)", _run_store._persistence.path)
    except Exception as exc:
        logger.warning("Run persistence unavailable, runs will not survive restart: %s", exc)
    try:
        import ApplicationServices
        opts = {"AXTrustedCheckOptionPrompt": False}
        _ax_permission_granted = ApplicationServices.AXIsProcessTrustedWithOptions(opts)
        if not _ax_permission_granted:
            logger.warning(
                "Accessibility permissions not granted. AX tree features will fail. "
                "Grant access in System Settings → Privacy & Security → Accessibility."
            )
    except Exception:
        _ax_permission_granted = False


async def _register_startup_adapters() -> None:
    """
    Eagerly register all available adapters into the executor waterfall at startup.
    Order follows ADAPTER_WATERFALL: extension → cdp → playwright → pyautogui → accessibility.
    """
    import sys as _sys2, os as _os2, logging as _log2
    _sys2.path.insert(0, _os2.path.join(_os2.path.dirname(__file__), ".."))
    _logger = _log2.getLogger(__name__)

    try:
        from core.computer_use_executor import get_executor
        executor = get_executor()

        # --- Extension adapter (browser.extension) ---
        # Connects via HTTP relay :3012 → Desktop app → TCP 3011 → Chrome extension.
        # Always attempt registration; health_check will gate usage at dispatch time.
        try:
            from adapters.browser.extension_adapter import ExtensionAdapter
            if "browser.extension" not in executor.registered_adapters():
                ext = ExtensionAdapter()
                await ext.initialize()
                executor.register("browser.extension", ext)
        except Exception as exc:
            _logger.warning("[startup] Extension adapter registration failed: %s", exc)

        # --- CDP adapter (browser.cdp) — only if Chrome is already running ---
        _cdp_port = int(_os2.environ.get("ACU_CDP_PORT", "9222"))
        try:
            import socket as _sock
            with _sock.create_connection(("127.0.0.1", _cdp_port), timeout=1.0):
                pass
            from adapters.browser.cdp_adapter import PlaywrightCDPAdapter
            if "browser.cdp" not in executor.registered_adapters():
                cdp = PlaywrightCDPAdapter(port=_cdp_port)
                await cdp.initialize()
                executor.register("browser.cdp", cdp)
        except Exception:
            pass  # Chrome not running — skip; waterfall falls to playwright

        # --- Remote CDP adapter (browser.remote-cdp) — drives Chrome in remote desktops ---
        try:
            from adapters.browser.remote_cdp_adapter import RemoteCDPAdapter
            if "browser.remote-cdp" not in executor.registered_adapters():
                executor.register("browser.remote-cdp", RemoteCDPAdapter())
        except Exception as exc:
            _logger.warning("[startup] Remote CDP adapter registration failed: %s", exc)

        # --- PyAutoGUI desktop adapter (desktop.pyautogui) ---
        try:
            from adapters.desktop.pyautogui.pyautogui_adapter import PyAutoGUIAdapter
            if "desktop.pyautogui" not in executor.registered_adapters():
                executor.register("desktop.pyautogui", PyAutoGUIAdapter())
        except Exception:
            pass  # pyautogui not installed or not on a desktop environment

        # --- Accessibility adapter (desktop.accessibility) ---
        try:
            from adapters.desktop.accessibility_adapter import AccessibilityAdapter
            if "desktop.accessibility" not in executor.registered_adapters():
                executor.register("desktop.accessibility", AccessibilityAdapter())
        except Exception:
            pass

    except ImportError:
        pass  # executor module unavailable — /v1/computer will handle lazily


@app.on_event("shutdown")
async def shutdown_event():
    """Cleanup on shutdown."""
    try:
        from canonical_router import shutdown_canonical_service
        await shutdown_canonical_service()
    except Exception as exc:
        logger.warning("Canonical computer service shutdown failed: %s", exc)
    await session_manager.shutdown()


from computer_use_router import router as computer_use_router
app.include_router(computer_use_router)
from browser_skills_router import router as browser_skills_router
app.include_router(browser_skills_router)
from network_traces_router import router as network_traces_router
app.include_router(network_traces_router)
from canonical_router import router as canonical_computer_router
app.include_router(canonical_computer_router)
from cloud_credentials_router import router as cloud_credentials_router
app.include_router(cloud_credentials_router)
from toolset_browser import router as toolset_browser_router
app.include_router(toolset_browser_router)

# Demo UI — mounted only when started via the demo launcher (demo.py sets
# ALLTERNIT_ACU_DEMO=1). Serves the self-contained page at GET /demo.
if os.environ.get("ALLTERNIT_ACU_DEMO") == "1":
    from demo_ui import router as demo_ui_router
    app.include_router(demo_ui_router)

# ---------------------------------------------------------------------------
# /v1/computer — compatibility alias for the old Claude-native payload
#
# Old callers (acu_mcp, the JS SDK's executeCompatibilityAction, agent-swarm)
# send {action, coordinate, text, key, delta, url, ...}. That payload is turned
# into one allternit.computer.v1 / allternit.browser.v1 contract call
# (core/toolset_executor.alias_v1_computer) and run on the shared executor,
# allternit-api POST /api/v1/computers/:id/toolset, when the engine runs next
# to it. Standalone (no ALLTERNIT_API_URL) it runs on the in-process adapter
# waterfall. The executor reaches browser sessions through /v1/execute, never
# through this route, so there is no executor -> gateway -> executor loop.
# ---------------------------------------------------------------------------

import sys as _sys, os as _os
_sys.path.insert(0, _os.path.join(_os.path.dirname(__file__), ".."))

try:
    from core.computer_use_executor import ComputerUseExecutor, get_executor, NATIVE_CLAUDE_ACTIONS, BROWSER_EXTENSION_ACTIONS
    from core.base_adapter import ActionRequest as _ActionRequest
    _executor_available = True
except ImportError:
    _executor_available = False

from core.toolset_executor import (
    ToolsetExecutorClient as _ToolsetExecutorClient,
    alias_v1_computer as _alias_v1_computer,
    executor_configured as _toolset_executor_configured,
    legacy_request as _legacy_request,
)


class ComputerToolRequest(BaseModel):
    """The old Claude native computer_use payload (kept for old callers)."""
    action: str
    session_id: str = Field(default_factory=lambda: f"sess-{uuid4().hex[:8]}")
    run_id: str = Field(default_factory=lambda: f"cu-{uuid4().hex[:12]}")
    coordinate: list[int] | None = None
    text: str | None = None
    key: str | None = None
    delta: list[int] | None = None
    url: str | None = None
    selector: str | None = None
    adapter_preference: str | None = None
    parameters: dict[str, Any] = Field(default_factory=dict)
    # Optional: which computer the executor runs on (default this-device) and
    # whose it is (the executor checks ownership, lease and approval).
    computer_id: str | None = None
    user_id: str | None = None


def _toolset_reply_to_legacy(toolset: str, member: str, call_input: dict[str, Any], reply: Any) -> dict[str, Any]:
    body = reply.body
    out: dict[str, Any] = {
        "status": "completed" if reply.ok else "failed",
        "action_type": member,
        "toolset": toolset,
        "member": member,
        "input": call_input,
        "is_error": not reply.ok,
        "content": body.get("content") or [],
        "screen": body.get("screen"),
        "browser_state": body.get("browser_state"),
        "extracted_content": {"text": reply.text()},
    }
    image = reply.image_b64()
    if image:
        out["extracted_content"]["data_url"] = f"data:image/png;base64,{image}"
    if not reply.ok:
        out["error"] = body.get("error") or reply.text()
        if reply.approval_id:
            out["approval_id"] = reply.approval_id
    return out


@app.post("/v1/computer")
async def computer_tool(req: ComputerToolRequest) -> dict[str, Any]:
    """Old Claude-native computer tool payload -> one contract call."""
    toolset, member, call_input = _alias_v1_computer(req.model_dump())
    if _toolset_executor_configured() or req.computer_id:
        client = _ToolsetExecutorClient(
            req.computer_id or "this-device",
            user_id=req.user_id,
            browser_session_id=req.session_id,
        )
        reply = await client.run(toolset, member, call_input, run_id=req.run_id)
        return _toolset_reply_to_legacy(toolset, member, call_input, reply)

    if not _executor_available:
        raise HTTPException(status_code=503, detail="ComputerUseExecutor not available")
    action_type, target, params = _legacy_request(toolset, member, call_input)
    if req.selector:
        params["selector"] = req.selector
        target = target or req.selector
    action = _ActionRequest(action_type=action_type, target=target, parameters=params)
    result = await get_executor().execute(
        action,
        session_id=req.session_id,
        run_id=req.run_id,
        adapter_preference=req.adapter_preference,
    )
    out = result.to_dict()
    out.update({"toolset": toolset, "member": member, "input": call_input})
    return out


# ── /v1/run/parallel ─────────────────────────────────────────────────────────

class ParallelTaskBody(BaseModel):
    task_id: str
    goal: str
    url: Optional[str] = None
    family: str = "browser"
    adapter_id: str = "browser.playwright"
    timeout_ms: int = 30000


class ParallelRunRequest(BaseModel):
    tasks: List[ParallelTaskBody]
    max_concurrent: int = 4


@app.post("/v1/run/parallel")
async def run_parallel(body: ParallelRunRequest) -> dict[str, Any]:
    """
    Fan N tasks out across isolated browser/desktop adapter instances and aggregate results.

    Body:
      tasks: [{task_id, goal, url?, family, adapter_id, timeout_ms?}]
      max_concurrent: int (default 4)

    Response:
      {results: [...], total: N, succeeded: N, failed: N}
    """
    import sys as _sys_par, os as _os_par
    _sys_par.path.insert(0, _os_par.path.join(_os_par.path.dirname(__file__), ".."))

    from core.parallel_coordinator import ParallelTask, get_coordinator

    tasks = [
        ParallelTask(
            task_id=t.task_id,
            goal=t.goal,
            url=t.url,
            family=t.family,
            adapter_id=t.adapter_id,
            timeout_ms=t.timeout_ms,
        )
        for t in body.tasks
    ]

    coordinator = get_coordinator(max_concurrent=body.max_concurrent)
    results = await coordinator.run(tasks)

    serialized = [
        {
            "task_id": r.task_id,
            "session_id": r.session_id,
            "success": r.success,
            "result": r.result,
            "error": r.error,
            "duration_ms": r.duration_ms,
        }
        for r in results
    ]

    succeeded = sum(1 for r in results if r.success)
    return {
        "results": serialized,
        "total": len(results),
        "succeeded": succeeded,
        "failed": len(results) - succeeded,
    }


# ── /v1/run/hybrid ────────────────────────────────────────────────────────────

class HybridStepBody(BaseModel):
    surface: str  # "browser" or "desktop"
    action: str
    params: dict[str, Any] = Field(default_factory=dict)


class HybridRunRequest(BaseModel):
    steps: List[HybridStepBody]
    session_id: str = Field(default_factory=lambda: f"hybrid-{uuid4().hex[:8]}")


@app.post("/v1/run/hybrid")
async def run_hybrid(body: HybridRunRequest) -> dict[str, Any]:
    """
    Execute a sequence of browser and/or desktop steps in a single session.

    Body:
      steps: [{surface: "browser"|"desktop", action: "...", params: {...}}]
      session_id: str (optional; auto-generated if omitted)

    Response:
      {results: [...], success: bool}
    """
    import sys as _sys_hyb, os as _os_hyb
    _sys_hyb.path.insert(0, _os_hyb.path.join(_os_hyb.path.dirname(__file__), ".."))

    from adapters.desktop.hybrid_adapter import HybridAdapter, HybridStep

    steps = [
        HybridStep(surface=s.surface, action=s.action, params=s.params)
        for s in body.steps
    ]

    adapter = HybridAdapter()
    results = await adapter.execute_sequence(steps, session_id=body.session_id)
    overall_success = all(r.get("success", False) for r in results)

    return {"results": results, "success": overall_success}


# ── /v1/conformance ──────────────────────────────────────────────────────────

_conformance_results: dict[str, Any] = {}  # key = "suite_id:adapter_id"


class ConformanceRunRequest(BaseModel):
    adapter_id: Optional[str] = None  # override default adapter for the suite


@app.get("/v1/conformance/results")
async def conformance_results() -> dict[str, Any]:
    """Return all stored conformance suite results."""
    from conformance import ConformanceRunner
    from conformance.suites import build_all_suites

    matrix = [
        {
            "suite_id": v["suite_id"],
            "adapter_id": v["adapter_id"],
            "grade": v["grade"],
            "pass_rate": v["pass_rate"],
            "total": v["total"],
            "passed": v["passed"],
            "failed": v["failed"],
            "timestamp": v["timestamp"],
        }
        for v in _conformance_results.values()
    ]
    suites = build_all_suites()
    return {
        "results": matrix,
        "suites": [{"suite_id": s.suite_id, "name": s.name, "test_count": len(s.list_tests())} for s in suites],
    }


@app.post("/v1/conformance/run/{suite_label}")
async def conformance_run(suite_label: str, body: ConformanceRunRequest = ConformanceRunRequest()) -> dict[str, Any]:
    """Run a conformance suite and store results.

    suite_label: A (browser), D (desktop), F (routing), R (retrieval), DX (desktop-ext), V (vision), PL (plugins), H (hybrid)
    """
    from conformance.suites import build_suite_a, build_suite_d, build_suite_f, build_suite_r, build_suite_dx, build_suite_h, build_suite_v, build_suite_pl

    SUITE_MAP = {
        "a": build_suite_a,
        "d": build_suite_d,
        "f": build_suite_f,
        "r": build_suite_r,
        "dx": build_suite_dx,
        "h": build_suite_h,
        "v": build_suite_v,
        "pl": build_suite_pl,
    }
    label = suite_label.lower()
    builder = SUITE_MAP.get(label)
    if not builder:
        raise HTTPException(status_code=404, detail=f"Unknown suite '{suite_label}'. Valid: {list(SUITE_MAP.keys())}")

    suite = builder()

    # Pick an adapter appropriate for the suite
    adapter = _get_conformance_adapter(label, body.adapter_id)
    if adapter is None:
        raise HTTPException(status_code=503, detail=f"No adapter available for suite '{suite_label}'")

    suite_result = await suite.run(adapter)

    key = f"{suite_result.suite_id}:{suite_result.adapter_id}"
    _conformance_results[key] = {
        "suite_id": suite_result.suite_id,
        "adapter_id": suite_result.adapter_id,
        "grade": suite_result.grade,
        "pass_rate": suite_result.pass_rate,
        "total": suite_result.total,
        "passed": suite_result.passed,
        "failed": suite_result.failed,
        "errors": suite_result.errors,
        "timestamp": suite_result.timestamp,
        "results": [r.to_dict() for r in suite_result.results],
    }

    return _conformance_results[key]


def _get_conformance_adapter(label: str, override_id: Optional[str] = None):
    """Instantiate the right adapter for a conformance suite label."""
    try:
        if label in ("a", "v"):
            from adapters.browser.playwright import PlaywrightAdapter
            return PlaywrightAdapter()
        elif label == "r":
            from adapters.browser.retrieval import RetrievalAdapter
            return RetrievalAdapter()
        elif label in ("d", "dx"):
            from adapters.desktop.accessibility_adapter import AccessibilityAdapter
            return _DesktopConformanceAdapter(AccessibilityAdapter())
        elif label == "f":
            return _DummyAdapter("policy.router")
        elif label == "h":
            from adapters.desktop.hybrid_adapter import HybridAdapter
            return HybridAdapter()
        elif label == "pl":
            return _DummyAdapter("plugins.registry")
        else:
            return None
    except Exception as e:
        logger.warning("Conformance adapter init failed for %s: %s", label, e)
        return None


class _DummyAdapter:
    """Minimal adapter stub for suites that don't need real browser/desktop I/O."""
    def __init__(self, adapter_id: str):
        self.adapter_id = adapter_id

    async def execute(self, *args, **kwargs):
        raise NotImplementedError("DummyAdapter.execute should not be called by this suite")


class _DesktopConformanceAdapter:
    """
    Bridges AccessibilityAdapter (execute(action, params) -> dict)
    to the BaseAdapter interface (execute(req, session_id, run_id) -> ResultEnvelope)
    expected by conformance suites D and DX.
    """

    def __init__(self, inner):
        self._inner = inner

    @property
    def adapter_id(self) -> str:
        return getattr(self._inner, "ADAPTER_ID", "desktop.accessibility")

    # Map generic action names to accessibility adapter command names
    _ACTION_MAP = {
        "screenshot": "take_screenshot",
        "observe": None,          # synthesized below
        "list_windows": "list_windows",
        "type_text": "type_text",
        "scroll": "scroll",
        "capture_region": "take_screenshot",
        "clipboard_write": "set_clipboard",
        "clipboard_read": "get_clipboard",
        "move_mouse": "hover",
    }

    async def execute(self, action_req, session_id: str = "", run_id: str = "") -> Any:
        from core import ResultEnvelope, Artifact
        import time

        action_type = getattr(action_req, "action_type", "screenshot")
        params = dict(getattr(action_req, "parameters", {}) or {})
        target = getattr(action_req, "target", "")
        now = datetime.utcnow().isoformat()

        start = time.monotonic()
        try:
            if action_type == "observe":
                # Synthesize: take screenshot + list windows for state summary
                sc = await self._inner.execute("take_screenshot", {})
                win = await self._inner.execute("list_windows", {})
                import pyautogui
                sw, sh = pyautogui.size()
                mx, my = pyautogui.position()
                raw = {
                    "success": True,
                    "screen_size": {"width": sw, "height": sh},
                    "mouse_position": {"x": mx, "y": my},
                    "windows": win.get("windows", []),
                    "image_b64": sc.get("image_b64"),
                }
            elif action_type == "move_mouse":
                mapped = self._ACTION_MAP.get(action_type, action_type)
                raw = await self._inner.execute(mapped, params)
                # Augment with confirmed position after move
                import pyautogui
                mx, my = pyautogui.position()
                raw = {**raw, "x": mx, "y": my, "position": {"x": mx, "y": my}}
            else:
                mapped = self._ACTION_MAP.get(action_type, action_type)
                raw = await self._inner.execute(mapped, params)
        except Exception as exc:
            return ResultEnvelope(
                run_id=run_id,
                session_id=session_id,
                adapter_id=self.adapter_id,
                family="desktop",
                mode="execute",
                action=action_type,
                target=target,
                status="failed",
                error={"code": "ADAPTER_ERROR", "message": str(exc)},
                artifacts=[],
                receipts=[],
                extracted_content=None,
                started_at=now,
                completed_at=now,
            )

        duration_ms = int((time.monotonic() - start) * 1000)
        completed_at = datetime.utcnow().isoformat()
        success = raw.get("success", True) if isinstance(raw, dict) else True

        # Screenshots become Artifact instances (key is image_b64 in this adapter)
        screenshot_b64 = (raw.get("image_b64") or raw.get("image_base64") or raw.get("screenshot")) if isinstance(raw, dict) else None
        artifacts = []
        if screenshot_b64:
            artifacts = [Artifact(
                type="screenshot",
                path="",
                media_type="image/png",
                size_bytes=len(screenshot_b64) * 3 // 4,
            )]

        extracted = {k: v for k, v in raw.items() if k != "image_b64"} if isinstance(raw, dict) else raw

        receipt_row = {"action": action_type, "timestamp": completed_at, "success": success}

        return ResultEnvelope(
            run_id=run_id,
            session_id=session_id,
            adapter_id=self.adapter_id,
            family="desktop",
            mode="execute",
            action=action_type,
            target=target,
            status="completed" if success else "failed",
            artifacts=artifacts,
            receipts=[receipt_row],
            extracted_content=extracted,
            started_at=now,
            completed_at=completed_at,
            duration_ms=duration_ms,
        )


if __name__ == "__main__":
    import os
    import uvicorn
    port = int(os.environ.get("ACU_GATEWAY_PORT", "8760"))
    uvicorn.run(app, host="127.0.0.1", port=port)
