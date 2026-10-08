"""The contract browser toolset (allternit.browser.v1) over the gateway's
Playwright sessions.

allternit-api's executor (cmd/allternit-api/src/computer_toolset.rs) does the
contract validation, lease, approval, audit and coordinate scaling. This
module runs one member against a session's tabs and returns the tab
inventory (`browser_state`) with every reply, success or failure.

POST /v1/toolset/browser
  {session_id, member, input, public_only}
  -> {is_error, text?, image?, browser_state: {tabs, state_changes}}

Tabs: every page in the session's BrowserContext is a tab with a stable
`tab_N` id. Exactly one tab is active (SessionInfo.current_page, which the
legacy /v1/execute handlers also use). A session always has at least one tab.

Element refs: `read_page` and `find` tag elements with
`data-allternit-ref="ref_N"`. A ref is scoped to its tab and goes stale after
navigation or a re-render that drops the element.

state_changes lists what this call changed, in order: `tab_opened`,
`tab_closed`, `active_tab_changed` and `navigated` (the URL of a tab changed).

URL policy: only http(s) and about:blank are navigable. With `public_only`
(cloud targets) every request the page makes, redirects and subresources
included, is refused when its host is loopback, private, link-local, a cloud
metadata address or otherwise not public.
"""

from __future__ import annotations

import asyncio
import base64
import ipaddress
import os
import re
import socket
import tempfile
import time
from collections import deque
from pathlib import Path
from typing import Any, Optional
from urllib.parse import urlparse

from fastapi import APIRouter
from pydantic import BaseModel, Field

from session_manager import session_manager

router = APIRouter()

REF_RE = re.compile(r"^ref_\d+$")
LOG_LIMIT = 300
TEXT_LIMIT = 50_000
METADATA_HOSTS = {"metadata.google.internal", "metadata", "instance-data", "instance-data.ec2.internal"}


class ToolError(Exception):
    pass


class ToolsetBrowserRequest(BaseModel):
    session_id: str
    member: str
    input: dict[str, Any] = Field(default_factory=dict)
    run_id: Optional[str] = None
    public_only: bool = False


# ---------------------------------------------------------------------------
# URL policy
# ---------------------------------------------------------------------------


def scheme_allowed(url: str) -> bool:
    if url.strip().lower() == "about:blank":
        return True
    return urlparse(url).scheme.lower() in ("http", "https")


def ip_is_public(ip: str) -> bool:
    try:
        addr = ipaddress.ip_address(ip.split("%")[0])
    except ValueError:
        return False
    if isinstance(addr, ipaddress.IPv6Address) and addr.ipv4_mapped:
        addr = addr.ipv4_mapped
    return addr.is_global and not addr.is_multicast


_host_cache: dict[str, tuple[bool, float]] = {}


async def host_is_public(host: str) -> bool:
    host = (host or "").strip("[]").lower().rstrip(".")
    if not host or host in METADATA_HOSTS or host == "localhost" or host.endswith((".localhost", ".local", ".internal")):
        return False
    try:
        ipaddress.ip_address(host.split("%")[0])
        return ip_is_public(host)
    except ValueError:
        pass
    hit = _host_cache.get(host)
    if hit and time.time() - hit[1] < 60:
        return hit[0]
    try:
        infos = await asyncio.get_running_loop().getaddrinfo(host, None, type=socket.SOCK_STREAM)
        ok = bool(infos) and all(ip_is_public(info[4][0]) for info in infos)
    except OSError:
        ok = False
    _host_cache[host] = (ok, time.time())
    return ok


async def url_refusal(url: str, public_only: bool) -> Optional[str]:
    if not scheme_allowed(url):
        return f"{url} isn't allowed: only http(s) pages and about:blank can be opened."
    if public_only and url.strip().lower() != "about:blank":
        host = urlparse(url).hostname or ""
        if not await host_is_public(host):
            return f"{url} isn't allowed: cloud browsers can't open loopback, private-network or metadata addresses."
    return None


# ---------------------------------------------------------------------------
# Per-session tab registry
# ---------------------------------------------------------------------------


class SessionTabs:
    def __init__(self, context: Any):
        self.context = context
        self.seq = 0
        self.pages: dict[str, Any] = {}
        self.ids: dict[int, str] = {}
        self.console: dict[str, deque] = {}
        self.network: dict[str, deque] = {}
        self.guarded = False
        self.blocked: deque = deque(maxlen=20)

    def tab_id(self, page: Any) -> str:
        tid = self.ids.get(id(page))
        if tid and self.pages.get(tid) is page:
            return tid
        self.seq += 1
        tid = f"tab_{self.seq}"
        self.ids[id(page)] = tid
        self.pages[tid] = page
        self.console[tid] = deque(maxlen=LOG_LIMIT)
        self.network[tid] = deque(maxlen=LOG_LIMIT)
        self._listen(page, tid)
        return tid

    def _listen(self, page: Any, tid: str) -> None:
        console, network = self.console[tid], self.network[tid]

        def on_console(msg: Any) -> None:
            loc = getattr(msg, "location", None) or {}
            where = f" ({loc.get('url', '')}:{loc.get('lineNumber', '')})" if loc.get("url") else ""
            console.append(f"[{msg.type}] {msg.text}{where}")

        def on_error(err: Any) -> None:
            console.append(f"[pageerror] {err}")

        def on_response(resp: Any) -> None:
            req = resp.request
            network.append(f"{req.method} {resp.status} {req.resource_type} {req.url[:500]}")

        def on_failed(req: Any) -> None:
            failure = req.failure or "failed"
            network.append(f"{req.method} FAILED({failure}) {req.resource_type} {req.url[:500]}")

        page.on("console", on_console)
        page.on("pageerror", on_error)
        page.on("response", on_response)
        page.on("requestfailed", on_failed)

    def live(self) -> list[tuple[str, Any]]:
        out = []
        for page in self.context.pages:
            if not page.is_closed():
                out.append((self.tab_id(page), page))
        for tid in [t for t, p in self.pages.items() if p.is_closed()]:
            self.pages.pop(tid, None)
        return out


_registry: dict[str, SessionTabs] = {}


async def open_session(session_id: str, public_only: bool) -> tuple[Any, SessionTabs]:
    await session_manager.get_or_create_session(session_id)
    info = session_manager._sessions[session_id]
    tabs = _registry.get(session_id)
    if tabs is None or tabs.context is not info.context:
        tabs = SessionTabs(info.context)
        _registry[session_id] = tabs
    if public_only and not tabs.guarded:
        async def guard(route: Any) -> None:
            url = route.request.url
            scheme = urlparse(url).scheme.lower()
            if scheme in ("http", "https", "ws", "wss"):
                if not await host_is_public(urlparse(url).hostname or ""):
                    tabs.blocked.append(url)
                    await route.abort("blockedbyclient")
                    return
            await route.continue_()

        await info.context.route("**/*", guard)
        tabs.guarded = True
    return info, tabs


def ensure_active(info: Any, tabs: SessionTabs) -> None:
    live = tabs.live()
    if info.current_page is None or info.current_page.is_closed():
        info.current_page = live[-1][1] if live else None


def snapshot(info: Any, tabs: SessionTabs) -> dict[str, Any]:
    return {tid: page.url for tid, page in tabs.live()} | {"__active__": tabs.tab_id(info.current_page) if info.current_page else ""}


async def browser_state(info: Any, tabs: SessionTabs, before: dict[str, Any]) -> dict[str, Any]:
    ensure_active(info, tabs)
    if info.current_page is None:
        info.current_page = await info.context.new_page()
    active = tabs.tab_id(info.current_page)
    listing = []
    for tid, page in tabs.live():
        try:
            title = await asyncio.wait_for(page.title(), 2)
        except Exception:
            title = ""
        listing.append({"tab_id": tid, "title": title, "url": page.url, "active": tid == active})
    changes: list[dict[str, Any]] = []
    now = {t["tab_id"]: t["url"] for t in listing}
    for tid in now:
        if tid not in before:
            changes.append({"type": "tab_opened", "tab_id": tid, "url": now[tid]})
    for tid in before:
        if tid != "__active__" and tid not in now:
            changes.append({"type": "tab_closed", "tab_id": tid})
    if before.get("__active__") != active:
        changes.append({"type": "active_tab_changed", "tab_id": active})
    for tid, url in now.items():
        if tid in before and before[tid] != url:
            changes.append({"type": "navigated", "tab_id": tid, "url": url})
    return {"tabs": listing, "state_changes": changes}


# ---------------------------------------------------------------------------
# Page helpers
# ---------------------------------------------------------------------------

PW_KEYS = {
    "ctrl": "Control", "control": "Control", "shift": "Shift", "alt": "Alt", "option": "Alt", "opt": "Alt",
    "cmd": "Meta", "command": "Meta", "super": "Meta", "meta": "Meta", "win": "Meta",
    "return": "Enter", "enter": "Enter", "kp_enter": "Enter", "esc": "Escape", "escape": "Escape",
    "backspace": "Backspace", "tab": "Tab", "delete": "Delete", "space": " ", "insert": "Insert",
    "page_down": "PageDown", "pagedown": "PageDown", "next": "PageDown",
    "page_up": "PageUp", "pageup": "PageUp", "prior": "PageUp", "home": "Home", "end": "End",
    "up": "ArrowUp", "down": "ArrowDown", "left": "ArrowLeft", "right": "ArrowRight",
}


def pw_key(part: str) -> str:
    p = part.strip()
    low = p.lower()
    if low in PW_KEYS:
        return PW_KEYS[low]
    if re.fullmatch(r"f([1-9]|1[0-2])", low):
        return low.upper()
    return p


def chord(spec: str) -> list[str]:
    return [pw_key(p) for p in spec.split("+") if p.strip()]


def modifiers_of(spec: Optional[str]) -> list[str]:
    mods = chord(spec or "")
    bad = [m for m in mods if m not in ("Control", "Shift", "Alt", "Meta")]
    if bad:
        raise ToolError(f"not a modifier key: {bad[0]}")
    return mods


def page_for(info: Any, tabs: SessionTabs, tab_id: Optional[str]) -> Any:
    if tab_id:
        page = tabs.pages.get(tab_id)
        if page is None or page.is_closed():
            raise ToolError(f"There's no open tab {tab_id}. Call list_tabs to see the open tabs.")
        return page
    ensure_active(info, tabs)
    if info.current_page is None:
        raise ToolError("This browser has no open tab.")
    return info.current_page


async def locate(page: Any, target: dict[str, Any]) -> Any:
    ref = str(target.get("ref", ""))
    if not REF_RE.match(ref):
        raise ToolError(f"{ref!r} isn't an element ref; refs look like ref_7 and come from read_page or find.")
    loc = page.locator(f'[data-allternit-ref="{ref}"]')
    if await loc.count() == 0:
        raise ToolError(f"{ref} isn't on this page any more (stale after navigation or a re-render). Call read_page or find again.")
    return loc.first


def point(target: Optional[dict[str, Any]]) -> tuple[float, float]:
    if not target or target.get("type") != "coordinate":
        raise ToolError("this member needs a coordinate target")
    return float(target.get("x", 0)), float(target.get("y", 0))


async def hold(page: Any, mods: list[str], down: bool) -> None:
    for m in mods if down else reversed(mods):
        await (page.keyboard.down(m) if down else page.keyboard.up(m))


INDEX_JS = r"""
({mode, filter, depth, ref, query}) => {
  window.__allternitRefSeq = window.__allternitRefSeq || 0;
  const SKIP = new Set(['SCRIPT','STYLE','NOSCRIPT','TEMPLATE','HEAD','META','LINK']);
  const INTERACTIVE_ROLES = new Set(['button','link','checkbox','radio','tab','menuitem','option','switch','textbox','combobox','slider','searchbox','spinbutton']);
  const LEAF = new Set(['link','button','heading','option','img','textbox','searchbox','combobox','slider','checkbox','radio','switch']);
  const roleOf = (el) => {
    const r = el.getAttribute('role'); if (r) return r.split(/\s+/)[0];
    const t = el.tagName.toLowerCase();
    if (t === 'a') return el.hasAttribute('href') ? 'link' : 'generic';
    if (t === 'button' || t === 'summary') return 'button';
    if (t === 'input') {
      const ty = (el.getAttribute('type') || 'text').toLowerCase();
      if (ty === 'hidden') return 'none';
      return ({checkbox:'checkbox', radio:'radio', button:'button', submit:'button', reset:'button', image:'button', file:'button', range:'slider', search:'searchbox', number:'spinbutton'})[ty] || 'textbox';
    }
    if (t === 'textarea') return 'textbox';
    if (t === 'select') return 'combobox';
    if (t === 'option') return 'option';
    if (/^h[1-6]$/.test(t)) return 'heading';
    if (t === 'img' || t === 'svg') return 'img';
    return ({nav:'navigation', main:'main', ul:'list', ol:'list', li:'listitem', table:'table', tr:'row', td:'cell', th:'columnheader', form:'form', dialog:'dialog', p:'paragraph', label:'label', header:'banner', footer:'contentinfo', aside:'complementary', article:'article', section:'region', iframe:'iframe'})[t] || 'generic';
  };
  const interactive = (el, role) => INTERACTIVE_ROLES.has(role) || el.isContentEditable || el.hasAttribute('onclick') || (el.hasAttribute('tabindex') && el.tabIndex >= 0);
  const clip = (s, n) => { s = (s || '').replace(/\s+/g, ' ').trim(); return s.length > n ? s.slice(0, n) + '…' : s; };
  const ownText = (el) => Array.from(el.childNodes).filter(n => n.nodeType === 3).map(n => n.textContent).join(' ');
  const nameOf = (el, role) => {
    const a = el.getAttribute('aria-label'); if (a) return a;
    const lb = el.getAttribute('aria-labelledby');
    if (lb) { const t = lb.split(/\s+/).map(id => (document.getElementById(id) || {}).innerText || '').join(' ').trim(); if (t) return t; }
    if (el.labels && el.labels.length) return Array.from(el.labels).map(l => l.innerText).join(' ');
    const alt = el.getAttribute('alt') || el.getAttribute('title') || el.getAttribute('placeholder'); if (alt) return alt;
    if (['textbox','searchbox','combobox','slider','spinbutton'].includes(role)) return '';
    return LEAF.has(role) ? (el.innerText || el.textContent || '') : ownText(el);
  };
  const visible = (el) => {
    const s = getComputedStyle(el);
    if (s.display === 'none' || s.visibility === 'hidden') return false;
    const r = el.getBoundingClientRect();
    return r.width > 0 || r.height > 0 || s.display === 'contents';
  };
  const inView = (el) => { const r = el.getBoundingClientRect(); return r.bottom > 0 && r.right > 0 && r.top < innerHeight && r.left < innerWidth; };
  const refFor = (el) => { let r = el.getAttribute('data-allternit-ref'); if (!r) { r = 'ref_' + (++window.__allternitRefSeq); el.setAttribute('data-allternit-ref', r); } return r; };
  const extra = (el, role) => {
    const bits = [];
    if ('value' in el && ['textbox','searchbox','combobox','slider','spinbutton'].includes(role) && el.value) bits.push('value=' + JSON.stringify(clip(String(el.value), 60)));
    if (role === 'checkbox' || role === 'radio' || role === 'switch') bits.push(el.checked || el.getAttribute('aria-checked') === 'true' ? 'checked' : 'unchecked');
    if (role === 'link' && el.getAttribute('href')) bits.push('href=' + JSON.stringify(clip(el.getAttribute('href'), 80)));
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') bits.push('disabled');
    if (/^H[1-6]$/.test(el.tagName)) bits.push('level=' + el.tagName[1]);
    return bits.length ? ' ' + bits.join(' ') : '';
  };
  let root = document.body || document.documentElement;
  if (ref) { root = document.querySelector('[data-allternit-ref="' + ref + '"]'); if (!root) return {error: 'stale'}; }
  if (mode === 'find') {
    const words = String(query || '').toLowerCase().split(/[^a-z0-9]+/).filter(w => w.length > 1);
    const hits = [];
    for (const el of root.querySelectorAll('*')) {
      if (SKIP.has(el.tagName) || !visible(el)) continue;
      const role = roleOf(el); if (role === 'none') continue;
      const isInt = interactive(el, role);
      const name = clip(nameOf(el, role), 100);
      if (!isInt && !name) continue;
      const hay = [role, name, el.getAttribute('type'), el.getAttribute('name'), el.id, el.getAttribute('placeholder')].join(' ').toLowerCase();
      let score = 0; for (const w of words) if (hay.includes(w)) score += 2;
      if (!score) continue;
      if (isInt) score += 1;
      if (inView(el)) score += 0.5;
      hits.push({el, role, name, score});
    }
    hits.sort((a, b) => b.score - a.score);
    return {lines: hits.slice(0, 20).map(h => {
      const r = h.el.getBoundingClientRect();
      return '- ' + h.role + (h.name ? ' ' + JSON.stringify(h.name) : '') + extra(h.el, h.role) + ' [' + refFor(h.el) + '] at (' + Math.round(r.left + r.width / 2) + ', ' + Math.round(r.top + r.height / 2) + ')';
    }), count: hits.length};
  }
  const lines = []; let truncated = false;
  const walk = (node, d, indent) => {
    for (const el of node.children) {
      if (lines.length >= 2000) { truncated = true; return; }
      if (SKIP.has(el.tagName) || !visible(el)) continue;
      const role = roleOf(el); if (role === 'none') continue;
      const isInt = interactive(el, role);
      const name = clip(nameOf(el, role), 100);
      let emit = filter === 'interactive' ? isInt : (role !== 'generic' || isInt || !!name);
      if (emit && filter !== 'all' && !inView(el)) emit = false;
      if (emit) lines.push('  '.repeat(indent) + '- ' + role + (name ? ' ' + JSON.stringify(name) : '') + extra(el, role) + ' [' + refFor(el) + ']');
      if (d + 1 < depth && !(emit && LEAF.has(role))) walk(el, d + 1, emit ? indent + 1 : indent);
      if (el.shadowRoot && d + 1 < depth) walk(el.shadowRoot, d + 1, emit ? indent + 1 : indent);
    }
  };
  walk(root, 0, 0);
  return {lines, truncated};
}
"""


async def page_index(page: Any, **kw: Any) -> dict[str, Any]:
    args = {"mode": "tree", "filter": None, "depth": 15, "ref": None, "query": None} | kw
    out = await page.evaluate(INDEX_JS, args)
    if out.get("error") == "stale":
        raise ToolError(f"{args['ref']} isn't on this page any more. Call read_page or find again.")
    return out


def upload_root() -> Path:
    return Path(os.environ.get("ALLTERNIT_BROWSER_UPLOAD_ROOT") or Path(tempfile.gettempdir()) / "allternit-browser-uploads").resolve()


# ---------------------------------------------------------------------------
# Members
# ---------------------------------------------------------------------------


async def run_member(info: Any, tabs: SessionTabs, member: str, inp: dict[str, Any], public_only: bool) -> dict[str, Any]:
    if member == "new_tab":
        page = await info.context.new_page()
        info.current_page = page
        return {"text": f"Opened {tabs.tab_id(page)} and switched to it."}
    if member == "list_tabs":
        return {"text": "The open tabs are listed in browser_state."}
    if member == "switch_tab":
        page = page_for(info, tabs, inp.get("tab_id"))
        info.current_page = page
        await page.bring_to_front()
        return {"text": f"Switched to {inp['tab_id']}."}
    if member == "close_tab":
        page = page_for(info, tabs, inp.get("tab_id"))
        was_active = page is info.current_page
        await page.close()
        if was_active:
            info.current_page = None
            ensure_active(info, tabs)
        if info.current_page is None:
            info.current_page = await info.context.new_page()
        if was_active:
            await info.current_page.bring_to_front()
        return {"text": f"Closed {inp['tab_id']}."}

    page = page_for(info, tabs, inp.get("tab_id"))
    target = inp.get("target")
    mods = modifiers_of(inp.get("modifiers"))

    if member == "navigate":
        url = str(inp.get("url", "")).strip()
        if url in ("back", "forward", "reload"):
            await {"back": page.go_back, "forward": page.go_forward, "reload": page.reload}[url](wait_until="domcontentloaded", timeout=30_000)
        else:
            refusal = await url_refusal(url, public_only)
            if refusal:
                raise ToolError(refusal)
            blocked_before = len(tabs.blocked)
            try:
                await page.goto(url, wait_until="domcontentloaded", timeout=30_000)
            except Exception as e:
                if len(tabs.blocked) > blocked_before:
                    raise ToolError(f"{tabs.blocked[-1]} isn't allowed: cloud browsers can't open loopback, private-network or metadata addresses.")
                raise ToolError(f"Couldn't open {url}: {e}")
            if not scheme_allowed(page.url) and page.url != "about:blank":
                await page.goto("about:blank")
                raise ToolError(f"The page tried to move to {page.url}, which isn't allowed.")
        return {"text": f"Navigated to {page.url} ({await page.title()})."}
    if member in ("screenshot", "zoom"):
        png = await page.screenshot(type="png")
        return {"image": base64.b64encode(png).decode()}

    if member in ("left_click", "right_click", "middle_click", "double_click", "triple_click", "hover"):
        button = {"right_click": "right", "middle_click": "middle"}.get(member, "left")
        count = {"double_click": 2, "triple_click": 3}.get(member, 1)
        if isinstance(target, dict) and target.get("type") == "ref":
            loc = await locate(page, target)
            if member == "hover":
                await loc.hover(modifiers=mods or None, timeout=10_000)
            else:
                await loc.click(button=button, click_count=count, modifiers=mods or None, timeout=10_000)
            return {}
        x, y = point(target)
        await hold(page, mods, True)
        try:
            if member == "hover":
                await page.mouse.move(x, y)
            else:
                await page.mouse.click(x, y, button=button, click_count=count)
        finally:
            await hold(page, mods, False)
        return {}
    if member == "mouse_move":
        x, y = point(target)
        await page.mouse.move(x, y)
        return {}
    if member in ("left_mouse_down", "left_mouse_up"):
        x, y = point(target)
        await page.mouse.move(x, y)
        await (page.mouse.down() if member == "left_mouse_down" else page.mouse.up())
        return {}
    if member == "left_click_drag":
        fx, fy = point(inp.get("from"))
        tx, ty = point(target)
        await hold(page, mods, True)
        try:
            await page.mouse.move(fx, fy)
            await page.mouse.down()
            await page.mouse.move(tx, ty, steps=12)
            await page.mouse.up()
        finally:
            await hold(page, mods, False)
        return {}
    if member == "scroll":
        x, y = point(target)
        notches = max(1, min(10, round(float(inp.get("scroll_amount") or 3))))
        dx, dy = {"up": (0, -100), "down": (0, 100), "left": (-100, 0), "right": (100, 0)}[inp.get("scroll_direction") or "down"]
        await page.mouse.move(x, y)
        await hold(page, mods, True)
        try:
            for _ in range(notches):
                await page.mouse.wheel(dx, dy)
                await asyncio.sleep(0.03)
        finally:
            await hold(page, mods, False)
        return {}
    if member == "scroll_to":
        loc = await locate(page, target or {})
        await loc.scroll_into_view_if_needed(timeout=10_000)
        return {}
    if member == "type":
        await page.keyboard.type(str(inp.get("text", "")))
        return {}
    if member == "key":
        repeat = max(1, min(100, round(float(inp.get("repeat") or 1))))
        for _ in range(repeat):
            for combo in str(inp.get("text", "")).split():
                await page.keyboard.press("+".join(chord(combo)))
        return {}
    if member == "hold_key":
        keys = chord(str(inp.get("text", "")))
        seconds = max(0.0, min(30.0, float(inp.get("duration") or 1)))
        for k in keys:
            await page.keyboard.down(k)
        try:
            await asyncio.sleep(seconds)
        finally:
            for k in reversed(keys):
                await page.keyboard.up(k)
        return {}
    if member == "read_page":
        depth = max(1, min(40, int(inp.get("depth") or 15)))
        ref = inp.get("ref")
        if ref and not REF_RE.match(str(ref)):
            raise ToolError(f"{ref!r} isn't an element ref.")
        out = await page_index(page, filter=inp.get("filter"), depth=depth, ref=ref)
        body = "\n".join(out.get("lines") or []) or "(no visible elements)"
        if out.get("truncated"):
            body += "\n… truncated; pass ref to read a subtree, or filter: \"interactive\"."
        return {"text": f"Page: {await page.title()} ({page.url[:300]})\n{body}"[:TEXT_LIMIT]}
    if member == "find":
        out = await page_index(page, mode="find", query=str(inp.get("query", "")))
        lines = out.get("lines") or []
        if not lines:
            return {"text": f"Nothing on the page matches {inp.get('query')!r}. Try read_page."}
        return {"text": f"{out.get('count', len(lines))} matches, best first:\n" + "\n".join(lines)}
    if member == "get_page_text":
        body = await page.evaluate("() => (document.body && document.body.innerText) || ''")
        return {"text": f"Title: {await page.title()}\nURL: {page.url[:300]}\n\n{body}"[:TEXT_LIMIT]}
    if member == "form_input":
        loc = await locate(page, target or {})
        value = inp.get("value")
        kind = await loc.evaluate("(el) => el.tagName.toLowerCase() + ':' + ((el.getAttribute('type') || '').toLowerCase())")
        if kind.startswith("select:"):
            try:
                await loc.select_option(value=str(value), timeout=5_000)
            except Exception:
                await loc.select_option(label=str(value), timeout=5_000)
        elif kind in ("input:checkbox", "input:radio"):
            on = value if isinstance(value, bool) else str(value).lower() in ("true", "1", "on", "yes", "checked")
            await loc.set_checked(on, timeout=5_000)
        else:
            await loc.fill("" if value is None else str(value), timeout=5_000)
        return {}
    if member == "file_upload":
        if inp.get("document_ids"):
            raise ToolError("This browser can't fetch staged documents; pass file paths on the browser's computer instead.")
        paths = [str(p) for p in inp.get("paths") or []]
        if not paths:
            raise ToolError("paths is required")
        resolved = []
        for p in paths:
            rp = Path(p).expanduser().resolve()
            if public_only and upload_root() not in rp.parents:
                raise ToolError(f"{p} isn't in the upload folder ({upload_root()}); cloud browsers only upload files staged there.")
            if not rp.is_file():
                raise ToolError(f"{p} doesn't exist on the browser's computer.")
            resolved.append(str(rp))
        loc = await locate(page, target or {})
        await loc.set_input_files(resolved, timeout=10_000)
        return {"text": f"Uploaded {len(resolved)} file(s)."}
    if member in ("read_console", "read_network"):
        log = (tabs.console if member == "read_console" else tabs.network).get(tabs.tab_id(page)) or deque()
        entries = list(log)[-150:]
        label = "console messages" if member == "read_console" else "network requests"
        if not entries:
            return {"text": f"No {label} recorded on this tab yet (recording starts when the tab is first used)."}
        return {"text": f"Last {len(entries)} {label}:\n" + "\n".join(entries)[:TEXT_LIMIT]}
    if member == "javascript_exec":
        result = await page.evaluate("(code) => Promise.resolve((0, eval)(code)).then(v => { try { return JSON.stringify(v); } catch (e) { return String(v); } })", str(inp.get("text", "")))
        return {"text": "undefined" if result is None else str(result)[:20_000]}
    raise ToolError(f"{member} isn't a member of the browser toolset.")


@router.post("/v1/toolset/browser")
async def toolset_browser(req: ToolsetBrowserRequest) -> dict[str, Any]:
    try:
        info, tabs = await open_session(req.session_id, req.public_only)
    except Exception as e:
        return {
            "is_error": True,
            "text": f"The browser isn't available on this computer (Playwright couldn't start a session: {e}).",
            "browser_state": {"tabs": [], "state_changes": []},
        }
    ensure_active(info, tabs)
    if info.current_page is None:
        info.current_page = await info.context.new_page()
    before = snapshot(info, tabs)
    reply: dict[str, Any]
    try:
        reply = {"is_error": False, **await run_member(info, tabs, req.member, req.input, req.public_only)}
    except ToolError as e:
        reply = {"is_error": True, "text": str(e)}
    except Exception as e:  # Playwright errors: report to the model, keep the session
        reply = {"is_error": True, "text": f"{req.member} failed: {str(e).splitlines()[0] if str(e) else type(e).__name__}"}
    try:
        reply["browser_state"] = await browser_state(info, tabs, before)
    except Exception as e:
        reply["browser_state"] = {"tabs": [], "state_changes": [], "error": str(e)}
    return reply


__all__ = ["router", "url_refusal", "host_is_public", "scheme_allowed"]
