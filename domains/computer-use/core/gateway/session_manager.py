"""
Persistent Browser Session Manager for Allternit Computer Use Gateway

Manages Playwright browser contexts keyed by session_id.
Provides persistent sessions across multiple actions.
"""

from __future__ import annotations

import asyncio
import logging
import os
import re
import time
from pathlib import Path
from typing import Dict, Optional, Tuple, Any

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------- Files drive
#
# While the Files drive is on (`ALLTERNIT_FILES_DRIVE=1`, the switch
# allternit-api reads), every download in an agent browser session is saved
# into the drive's Downloads root, `~/Allternit/Downloads`, where the drive's
# watcher indexes it (contracts/files-drive SPEC section 3).
# `ALLTERNIT_FILES_DOWNLOADS_DIR` picks another folder. Off, downloads stay in
# Playwright's temporary folder as before. Downloaded files are untrusted:
# the Downloads root has no agent access, so a download never starts a task.

_UNSAFE_NAME_CHARS = re.compile(r'[\x00-\x1f\x7f/\\:*?"<>|]')
_MAX_NAME_CHARS = 120


def files_downloads_dir() -> Optional[Path]:
    """The folder agent-browser downloads are saved to, or None when off."""
    custom = os.environ.get("ALLTERNIT_FILES_DOWNLOADS_DIR", "").strip()
    if custom:
        return Path(custom).expanduser()
    if os.environ.get("ALLTERNIT_FILES_DRIVE", "").strip().lower() in ("1", "true"):
        return Path.home() / "Allternit" / "Downloads"
    return None


def safe_download_name(raw: Optional[str]) -> str:
    """A file name from a site's suggested name (untrusted): no path
    separators or control characters, no leading dot, at most 120
    characters with the extension kept."""
    name = _UNSAFE_NAME_CHARS.sub(" ", raw or "")
    name = " ".join(name.split()).lstrip(". ").strip()
    if len(name) > _MAX_NAME_CHARS:
        stem, dot, ext = name.rpartition(".")
        if dot and stem and 0 < len(ext) <= 16:
            name = stem[: _MAX_NAME_CHARS - len(ext) - 1].rstrip() + "." + ext
        else:
            name = name[:_MAX_NAME_CHARS].rstrip()
    return name or "download"


def reserve_download_path(directory: Path, name: str) -> Path:
    """Creates and returns a new empty file for `name` in `directory`:
    `name`, else `stem 2.ext`, `stem 3.ext`... Never reuses or follows an
    existing entry (O_EXCL), so a download never replaces a file."""
    stem, dot, ext = name.rpartition(".")
    if not dot or not stem:
        stem, ext = name, ""
    for n in range(1, 10_000):
        candidate = name if n == 1 else (f"{stem} {n}.{ext}" if ext else f"{stem} {n}")
        path = directory / candidate
        try:
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        except FileExistsError:
            continue
        os.close(fd)
        return path
    raise FileExistsError(f"no free name for {name!r} in {directory}")


async def save_download(download: Any, directory: Path) -> Optional[Path]:
    """Saves one Playwright download into `directory`. Best effort: a failed
    save is logged and the browser session carries on."""
    try:
        directory.mkdir(parents=True, exist_ok=True)
        target = reserve_download_path(directory, safe_download_name(getattr(download, "suggested_filename", None)))
        await download.save_as(str(target))
        return target
    except Exception as exc:  # noqa: BLE001 - never break the agent's session over a download
        logger.warning("files drive: download not saved to %s: %s", directory, exc)
        return None


def _route_downloads(context: Any) -> None:
    """Saves every page's downloads in `context` into the Downloads root."""
    directory = files_downloads_dir()
    if directory is None:
        return

    def on_download(download: Any) -> None:
        asyncio.ensure_future(save_download(download, directory))

    def on_page(page: Any) -> None:
        page.on("download", on_download)

    # `page` fires for every page the context opens, popups included.
    context.on("page", on_page)


class SessionInfo:
    """Information about a browser session."""
    
    def __init__(self, session_id: str, context: Any, page: Any):
        self.session_id = session_id
        self.context = context
        self.current_page = page
        self.created_at = time.time()
        self.last_accessed = time.time()
        self.action_count = 0
    
    def touch(self):
        """Update last accessed time."""
        self.last_accessed = time.time()
        self.action_count += 1
    
    @property
    def idle_time(self) -> float:
        """Seconds since last access."""
        return time.time() - self.last_accessed
    
    @property
    def age(self) -> float:
        """Seconds since creation."""
        return time.time() - self.created_at


class SessionManager:
    """
    Manages persistent browser sessions.
    
    Each session_id maps to:
    - One BrowserContext (isolated cookies/storage)
    - One current Page (active tab)
    """
    
    def __init__(
        self,
        idle_timeout: float = 1800,  # 30 minutes
        max_sessions: int = 10,
    ):
        self.idle_timeout = idle_timeout
        self.max_sessions = max_sessions
        
        self._playwright: Optional[Any] = None
        self._browser: Optional[Any] = None
        self._sessions: Dict[str, SessionInfo] = {}
        self._lock = asyncio.Lock()
        self._cleanup_task: Optional[asyncio.Task] = None
        self._initialized = False
    
    async def initialize(self) -> None:
        """Initialize Playwright and browser."""
        if self._initialized:
            return
        
        async with self._lock:
            if self._initialized:
                return
            
            skip_browser = os.environ.get("ALLTERNIT_ACU_SKIP_BROWSER", "").lower() in (
                "1",
                "true",
                "yes",
            )
            if not skip_browser:
                try:
                    from playwright.async_api import async_playwright

                    self._playwright = await async_playwright().start()
                    self._browser = await self._playwright.chromium.launch(
                        headless=True,
                        args=[
                            "--no-sandbox",
                            "--disable-dev-shm-usage",
                            "--disable-gpu",
                            "--disable-web-security",
                            "--disable-features=IsolateOrigins,site-per-process",
                        ],
                    )
                except Exception as exc:
                    import logging

                    logging.getLogger(__name__).warning(
                        "Playwright unavailable (%s); host screenshot fallback still works",
                        exc,
                    )
            self._initialized = True

            # Start background cleanup task
            self._cleanup_task = asyncio.create_task(self._cleanup_loop())
    
    async def _cleanup_loop(self) -> None:
        """Background task to clean up expired sessions."""
        while True:
            try:
                await asyncio.sleep(60)  # Check every minute
                await self.cleanup_expired()
            except asyncio.CancelledError:
                break
            except Exception:
                # Log error but keep running
                pass
    
    async def get_or_create_session(
        self,
        session_id: str,
    ) -> Tuple[Any, Any]:
        """
        Get existing session or create new one.
        
        Returns (context, current_page) for the session.
        """
        await self.initialize()
        
        async with self._lock:
            # Check for existing session
            if session_id in self._sessions:
                session = self._sessions[session_id]
                session.touch()
                
                # Verify page is still valid
                if session.current_page.is_closed():
                    # Create new page in same context
                    session.current_page = await session.context.new_page()
                
                return session.context, session.current_page
            
            # Check session limit
            if len(self._sessions) >= self.max_sessions:
                # Close oldest idle session
                await self._close_oldest_session()
            
            # Create new session
            context = await self._browser.new_context(
                viewport={"width": 1280, "height": 720},
                user_agent="Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
                accept_downloads=True,
            )
            _route_downloads(context)
            
            page = await context.new_page()
            
            session = SessionInfo(session_id, context, page)
            self._sessions[session_id] = session
            
            return context, page
    
    async def get_page(self, session_id: str) -> Optional[Page]:
        """
        Get current page for session.
        Returns None if session doesn't exist.
        """
        async with self._lock:
            if session_id not in self._sessions:
                return None
            
            session = self._sessions[session_id]
            session.touch()
            
            if session.current_page.is_closed():
                session.current_page = await session.context.new_page()
            
            return session.current_page
    
    async def close_session(self, session_id: str) -> bool:
        """
        Close a specific session.
        Returns True if session was found and closed.
        """
        async with self._lock:
            if session_id not in self._sessions:
                return False
            
            session = self._sessions.pop(session_id)
            
            try:
                await session.context.close()
            except Exception:
                pass
            
            return True
    
    async def cleanup_expired(self) -> int:
        """
        Close sessions that have been idle too long.
        Returns number of sessions closed.
        """
        closed_count = 0
        
        async with self._lock:
            now = time.time()
            expired = [
                sid for sid, s in self._sessions.items()
                if now - s.last_accessed > self.idle_timeout
            ]
            
            for sid in expired:
                session = self._sessions.pop(sid, None)
                if session:
                    try:
                        await session.context.close()
                    except Exception:
                        pass
                    closed_count += 1
        
        return closed_count
    
    async def _close_oldest_session(self) -> None:
        """Close the oldest idle session to make room."""
        if not self._sessions:
            return
        
        # Find oldest by last_accessed
        oldest_sid = min(
            self._sessions.keys(),
            key=lambda sid: self._sessions[sid].last_accessed
        )
        
        session = self._sessions.pop(oldest_sid, None)
        if session:
            try:
                await session.context.close()
            except Exception:
                pass
    
    async def get_session_stats(self) -> dict:
        """Get statistics about current sessions."""
        async with self._lock:
            return {
                "active_sessions": len(self._sessions),
                "max_sessions": self.max_sessions,
                "idle_timeout": self.idle_timeout,
                "sessions": [
                    {
                        "session_id": sid,
                        "action_count": s.action_count,
                        "idle_seconds": s.idle_time,
                        "age_seconds": s.age,
                    }
                    for sid, s in self._sessions.items()
                ]
            }
    
    async def shutdown(self) -> None:
        """Shutdown session manager and close all sessions."""
        # Stop cleanup task
        if self._cleanup_task:
            self._cleanup_task.cancel()
            try:
                await self._cleanup_task
            except asyncio.CancelledError:
                pass
        
        async with self._lock:
            # Close all sessions
            for session in self._sessions.values():
                try:
                    await session.context.close()
                except Exception:
                    pass
            
            self._sessions.clear()
            
            # Close browser
            if self._browser:
                try:
                    await self._browser.close()
                except Exception:
                    pass
                self._browser = None
            
            # Stop playwright
            if self._playwright:
                try:
                    await self._playwright.stop()
                except Exception:
                    pass
                self._playwright = None
            
            self._initialized = False


# Global session manager instance
session_manager = SessionManager()
