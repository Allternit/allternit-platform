"""Allternit Computer Use Adapters."""

ADAPTER_REGISTRY: dict[str, type] = {}

try:
    from .browser.playwright_adapter import PlaywrightAdapter
    ADAPTER_REGISTRY["browser.playwright"] = PlaywrightAdapter
except ImportError:
    pass

try:
    from .browser.webmcp import WebMcpAdapter
    ADAPTER_REGISTRY["browser.webmcp"] = WebMcpAdapter
except ImportError:
    pass

try:
    from .desktop.accessibility_adapter import AccessibilityAdapter
    ADAPTER_REGISTRY["desktop.accessibility"] = AccessibilityAdapter
except ImportError:
    pass

try:
    from .mobile.appagent_adapter import AppAgentAdapter
    ADAPTER_REGISTRY["mobile.appagent"] = AppAgentAdapter
except ImportError:
    pass

__all__ = ["ADAPTER_REGISTRY"]
