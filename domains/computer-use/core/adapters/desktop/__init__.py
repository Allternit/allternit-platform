"""Desktop automation adapters."""

try:
    from .accessibility_adapter import AccessibilityAdapter
except ImportError:
    AccessibilityAdapter = None  # type: ignore

__all__ = [
    "AccessibilityAdapter",
]
