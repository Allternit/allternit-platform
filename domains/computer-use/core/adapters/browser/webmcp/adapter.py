"""
Allternit Computer Use — WebMCP Adapter

Site-tool layer over the existing browser plumbing: a Python-side registry of
WebMCP-shaped site tools (derived from the gmail/github/notion plugin
manifests) plus an in-page bridge installing window.__allternitSiteTools.

Mirrors the TS site-tools layer in allternit-browser and follows the same
interface shape as the other browser adapters (initialize/close/navigate/capabilities/
health_check). Tool invocations are timed, redacted, and optionally recorded
into the run's JSONL recording as tool_call frames.
"""

from __future__ import annotations

import logging
import time
from typing import Any, Dict, List, Optional

from .adapters import load_default_site_tools
from .bridge import SiteToolsBridge
from .registry import (
    SiteTool,
    SiteToolDescriptor,
    SiteToolRegistry,
    SiteToolResult,
    WebMcpContext,
)

logger = logging.getLogger(__name__)

try:
    from playwright.async_api import async_playwright, Browser, BrowserContext, Page  # noqa: F401
    PLAYWRIGHT_AVAILABLE = True
except ImportError:
    PLAYWRIGHT_AVAILABLE = False
    Browser = Any  # type: ignore
    BrowserContext = Any  # type: ignore
    Page = Any  # type: ignore


class WebMcpAdapter:
    """
    WebMCP site-tools adapter (``browser.webmcp``).

    Owns a SiteToolRegistry loaded from the shared plugin manifests and,
    once a page is attached, a SiteToolsBridge injecting
    ``window.__allternitSiteTools`` into it. Blocked plugin actions are
    refused at the registry boundary before any browser primitive runs.
    """

    ADAPTER_ID = "browser.webmcp"

    def __init__(
        self,
        headless: bool = True,
        viewport: tuple = (1280, 720),
        timeout_ms: int = 30_000,
        plugin_ids: tuple = ("gmail", "github", "notion"),
        auto_launch: bool = False,
    ):
        if not PLAYWRIGHT_AVAILABLE:
            raise ImportError("playwright not installed: pip install playwright && playwright install chromium")
        self.headless = headless
        self.viewport = viewport
        self.timeout_ms = timeout_ms
        self.auto_launch = auto_launch

        self.registry = SiteToolRegistry()
        self.registry.register_all(load_default_site_tools(plugin_ids=plugin_ids))
        self._bridge: Optional[SiteToolsBridge] = None

        self._playwright = None
        self._browser: Optional[Browser] = None
        self._context: Optional[BrowserContext] = None
        self._page: Optional[Page] = None
        self._recorder: Optional[Any] = None  # core.action_recorder.ActionRecorder

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    async def initialize(self) -> None:
        if self._browser:
            return
        self._playwright = await async_playwright().start()
        self._browser = await self._playwright.chromium.launch(headless=self.headless)
        self._context = await self._browser.new_context(
            viewport={"width": self.viewport[0], "height": self.viewport[1]},
        )
        self._page = await self._context.new_page()
        self._page.set_default_timeout(self.timeout_ms)
        await self.attach_page(self._page)

    async def close(self) -> None:
        self._bridge = None
        self._page = None
        if self._browser:
            await self._browser.close()
            self._browser = None
        if self._playwright:
            await self._playwright.stop()
            self._playwright = None

    async def attach_page(self, page: "Page") -> SiteToolsBridge:
        """Attach a (CDP-connected) Playwright page and install the bridge."""
        page.set_default_timeout(self.timeout_ms)
        self._page = page
        self._bridge = SiteToolsBridge(page)
        await self._bridge.install(add_init_script=True)
        model_context = await self._bridge.probe_model_context()  # optional; logged only
        await self._bridge.register_tools(self.registry.descriptors())
        logger.info(
            "[webmcp] bridge installed on %s (%d tools, modelContext=%s)",
            getattr(page, "url", "?"),
            len(self.registry.descriptors()),
            model_context,
        )
        return self._bridge

    def set_recorder(self, recorder: Optional[Any]) -> None:
        """Attach a core.action_recorder.ActionRecorder for tool_call frames."""
        self._recorder = recorder

    # ------------------------------------------------------------------
    # Site tool surface
    # ------------------------------------------------------------------

    def list_tools(self) -> List[SiteToolDescriptor]:
        return self.registry.descriptors()

    def tools_for_origin(self, origin: Optional[str]) -> List[SiteTool]:
        return self.registry.tools_for_origin(origin)

    async def invoke_tool(
        self,
        name: str,
        args: Optional[Dict[str, Any]] = None,
        dry_run: bool = False,
    ) -> SiteToolResult:
        """
        Invoke a site tool through the registry. Blocked actions are refused
        at the tool boundary; latency is measured and the call is recorded
        (with redacted args) when a recorder is attached.
        """
        args = dict(args or {})
        ctx = WebMcpContext(page=self._page, dry_run=dry_run)
        start = time.monotonic()
        try:
            result = await self.registry.invoke(name, args, ctx)
        except Exception as exc:
            latency_ms = int((time.monotonic() - start) * 1000)
            await self._record(name, args, None, latency_ms, error=str(exc))
            return SiteToolResult(ok=False, summary=f"Error: {exc}", reason=str(exc))
        latency_ms = int((time.monotonic() - start) * 1000)
        await self._record(name, args, result, latency_ms, error=None)
        return result

    async def _record(
        self,
        name: str,
        args: Dict[str, Any],
        result: Optional[SiteToolResult],
        latency_ms: int,
        error: Optional[str],
    ) -> None:
        if self._recorder is None:
            return
        try:
            await self._recorder.record_tool_call(
                tool_name=name,
                args=args,
                result_summary=result.summary if result else None,
                latency_ms=latency_ms,
                error=error or (result.reason if result and not result.ok else None),
            )
        except Exception as exc:
            logger.warning("[webmcp] could not record tool call: %s", exc)

    # ------------------------------------------------------------------
    # Browser conveniences (same shape as the other browser adapters)
    # ------------------------------------------------------------------

    async def navigate(self, url: str) -> Dict[str, str]:
        if self._page is None:
            if not self.auto_launch:
                raise RuntimeError("no page attached; call initialize() or attach_page() first")
            await self.initialize()
        await self._page.goto(url, wait_until="domcontentloaded")
        return {"url": self._page.url, "title": await self._page.title()}

    async def get_url(self) -> str:
        if self._page is None:
            raise RuntimeError("no page attached")
        return self._page.url

    async def execute_js(self, code: str) -> Any:
        if self._page is None:
            raise RuntimeError("no page attached")
        return await self._page.evaluate(code)

    async def health_check(self) -> bool:
        try:
            if self._page is not None:
                await self._page.title()
                return True
            if self.auto_launch:
                await self.initialize()
                return self._page is not None
            return self.registry.list() != []
        except Exception:
            return False

    async def capabilities(self) -> Dict:
        return {
            "adapter_id": self.ADAPTER_ID,
            "dom_tree": False,
            "vision_required": False,
            "code_execution": False,
            "file_access": False,
            "auth_flows": True,
            "multi_tab": True,
            "platform": "any",
            "site_tools": len(self.registry.list()),
        }
