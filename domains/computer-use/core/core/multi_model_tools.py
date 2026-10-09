"""
Allternit Computer Use — Multi-Model Tool Definitions (Non-Claude Path)

Tool schemas handed to non-Claude models (GPT-4o, Gemini, Qwen, etc.) so they
can call into the ACU executor using their respective tool-use formats.

Each model family receives a version of the `computer` tool tailored to its
tool_use API shape. The executor (ComputerUseExecutor) handles all of them
identically once the call is decoded.

Supported formats:
  - OpenAI / Azure OpenAI  — function calling schema (gpt-4o, gpt-4-turbo, etc.)
  - Anthropic (non-native) — tool_use schema (claude-* without native computer tool)
  - Google Gemini          — FunctionDeclaration schema
  - Qwen / generic         — OpenAI-compatible function calling schema
"""

from __future__ import annotations

from typing import Any, Dict

# ---------------------------------------------------------------------------
# Shared action enum (same across all formats)
# ---------------------------------------------------------------------------

_ACTION_ENUM = [
    "screenshot",
    "left_click",
    "right_click",
    "middle_click",
    "double_click",
    "left_click_drag",
    "type",
    "key",
    "scroll",
    "cursor_position",
    "navigate",
    "extract",
    "fill",
    "wait",
    "tabs",
]

_ACTION_DESCRIPTIONS = {
    "screenshot": "Capture the current screen state",
    "left_click": "Left-click at [x, y] coordinates",
    "right_click": "Right-click at [x, y] coordinates",
    "middle_click": "Middle-click at [x, y] coordinates",
    "double_click": "Double-click at [x, y] coordinates",
    "left_click_drag": "Drag from [startX, startY] to [endX, endY]",
    "type": "Type text into the focused element",
    "key": "Press a key or key combo, e.g. 'Return', 'ctrl+c'",
    "scroll": "Scroll at [x, y] by [deltaX, deltaY] pixels",
    "cursor_position": "Return the current cursor position",
    "navigate": "Navigate the browser to a URL",
    "extract": "Extract text/HTML/JSON from the current page",
    "fill": "Fill an input field identified by CSS selector",
    "wait": "Wait for a specified number of milliseconds",
    "tabs": "List, create, close, or switch browser tabs",
}

# ---------------------------------------------------------------------------
# OpenAI function calling schema (gpt-4o, gpt-4-turbo, Qwen, etc.)
# ---------------------------------------------------------------------------

OPENAI_COMPUTER_TOOL: Dict[str, Any] = {
    "type": "function",
    "function": {
        "name": "computer",
        "description": (
            "Control the computer to accomplish tasks: take screenshots, click elements, "
            "type text, scroll, navigate the browser, extract page content, and more. "
            "After each action, call 'screenshot' to observe the result before deciding the next step."
        ),
        "parameters": {
            "type": "object",
            "required": ["action", "session_id"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": _ACTION_ENUM,
                    "description": "The action to perform. " + " | ".join(
                        f"{k}: {v}" for k, v in _ACTION_DESCRIPTIONS.items()
                    ),
                },
                "session_id": {
                    "type": "string",
                    "description": "Browser or desktop session ID",
                },
                "coordinate": {
                    "type": "array",
                    "items": {"type": "integer"},
                    "minItems": 2,
                    "maxItems": 2,
                    "description": "[x, y] screen coordinates (required for click/scroll/drag)",
                },
                "text": {
                    "type": "string",
                    "description": "Text to type or fill (for type/fill actions)",
                },
                "key": {
                    "type": "string",
                    "description": "Key name or combo to press, e.g. 'Return', 'ctrl+c'",
                },
                "delta": {
                    "type": "array",
                    "items": {"type": "integer"},
                    "minItems": 2,
                    "maxItems": 2,
                    "description": "[deltaX, deltaY] scroll offset in pixels",
                },
                "url": {
                    "type": "string",
                    "description": "URL to navigate to (for navigate action)",
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector for fill/extract actions",
                },
                "ms": {
                    "type": "integer",
                    "description": "Milliseconds to wait (for wait action)",
                },
                "adapter_preference": {
                    "type": "string",
                    "enum": [
                        "browser.extension",
                        "browser.cdp",
                        "browser.playwright",
                        "desktop.accessibility",
                    ],
                    "description": "Optional: pin a specific adapter (omit for automatic waterfall)",
                },
            },
        },
    },
}

# ---------------------------------------------------------------------------
# Anthropic tool_use schema (for claude-* without native computer tool beta)
# ---------------------------------------------------------------------------

ANTHROPIC_COMPUTER_TOOL: Dict[str, Any] = {
    "name": "computer",
    "description": (
        "Control the computer to accomplish tasks: take screenshots, click elements, "
        "type text, scroll, navigate the browser, extract page content, and more. "
        "After each action, call 'screenshot' to observe the result."
    ),
    "input_schema": {
        "type": "object",
        "required": ["action", "session_id"],
        "properties": {
            "action": {
                "type": "string",
                "enum": _ACTION_ENUM,
                "description": "Action to perform",
            },
            "session_id": {"type": "string"},
            "coordinate": {
                "type": "array",
                "items": {"type": "integer"},
                "description": "[x, y] for click/scroll",
            },
            "text": {"type": "string"},
            "key": {"type": "string"},
            "delta": {
                "type": "array",
                "items": {"type": "integer"},
                "description": "[deltaX, deltaY] for scroll",
            },
            "url": {"type": "string"},
            "selector": {"type": "string"},
            "ms": {"type": "integer"},
        },
    },
}

# ---------------------------------------------------------------------------
# Google Gemini FunctionDeclaration schema
# ---------------------------------------------------------------------------

GEMINI_COMPUTER_TOOL: Dict[str, Any] = {
    "function_declarations": [
        {
            "name": "computer",
            "description": (
                "Control the computer: take screenshots, click, type, scroll, "
                "navigate browser, extract page content."
            ),
            "parameters": {
                "type": "OBJECT",
                "required": ["action", "session_id"],
                "properties": {
                    "action": {
                        "type": "STRING",
                        "enum": _ACTION_ENUM,
                        "description": "Action to perform",
                    },
                    "session_id": {
                        "type": "STRING",
                        "description": "Session ID",
                    },
                    "coordinate": {
                        "type": "ARRAY",
                        "items": {"type": "INTEGER"},
                        "description": "[x, y] for click/scroll",
                    },
                    "text": {"type": "STRING"},
                    "key": {"type": "STRING"},
                    "delta": {
                        "type": "ARRAY",
                        "items": {"type": "INTEGER"},
                        "description": "[deltaX, deltaY] for scroll",
                    },
                    "url": {"type": "STRING"},
                    "selector": {"type": "STRING"},
                    "ms": {"type": "INTEGER"},
                },
            },
        }
    ]
}

# ---------------------------------------------------------------------------
# Factory
# ---------------------------------------------------------------------------

_TOOL_BY_PROVIDER = {
    "openai": OPENAI_COMPUTER_TOOL,
    "azure_openai": OPENAI_COMPUTER_TOOL,
    "qwen": OPENAI_COMPUTER_TOOL,
    "anthropic": ANTHROPIC_COMPUTER_TOOL,
    "gemini": GEMINI_COMPUTER_TOOL,
    "google": GEMINI_COMPUTER_TOOL,
}


def get_computer_tool(provider: str) -> Dict[str, Any]:
    """
    Return the computer tool definition for a given provider.

    provider: "openai" | "azure_openai" | "anthropic" | "gemini" | "qwen" | "google"
    Falls back to OpenAI format for unknown providers.
    """
    return _TOOL_BY_PROVIDER.get(provider.lower(), OPENAI_COMPUTER_TOOL)


def decode_tool_call(provider: str, tool_call: Dict[str, Any]) -> Dict[str, Any]:
    """
    Normalize a provider-specific tool call response to a flat kwargs dict
    that can be passed directly to the /v1/computer endpoint or executor.

    Returns a dict with keys matching ComputerToolRequest fields.
    """
    provider = provider.lower()

    if provider in ("openai", "azure_openai", "qwen"):
        import json as _json
        args = tool_call.get("function", {}).get("arguments", "{}")
        if isinstance(args, str):
            args = _json.loads(args)
        return args

    if provider == "anthropic":
        return tool_call.get("input", {})

    if provider in ("gemini", "google"):
        return dict(tool_call.get("args", {}))

    return tool_call
