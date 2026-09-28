"""
Allternit Computer Use Gateway — Self-Documenting Route Registry

Ported from background-computer-use's RouteRegistry.swift.
Exposes every route with field-level documentation via GET /v1/routes.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional


@dataclass
class FieldDescriptor:
    """Describes a single request or response field."""
    name: str
    type: str
    description: str
    required: bool = True
    default: Any = None

    def to_dict(self) -> Dict:
        return {
            "name": self.name,
            "type": self.type,
            "description": self.description,
            "required": self.required,
            "default": self.default,
        }


@dataclass
class RouteDescriptor:
    """Full descriptor for one API route."""
    route_id: str
    method: str  # GET | POST | DELETE
    path: str
    description: str
    request_fields: List[FieldDescriptor] = field(default_factory=list)
    response_fields: List[FieldDescriptor] = field(default_factory=list)
    tags: List[str] = field(default_factory=list)

    def to_dict(self) -> Dict:
        return {
            "route_id": self.route_id,
            "method": self.method,
            "path": self.path,
            "description": self.description,
            "request_fields": [f.to_dict() for f in self.request_fields],
            "response_fields": [f.to_dict() for f in self.response_fields],
            "tags": self.tags,
        }


# ---------------------------------------------------------------------------
# Common field sets (reused across routes)
# ---------------------------------------------------------------------------

_EXECUTE_REQUEST_FIELDS = [
    FieldDescriptor("action", "str", "Action type to perform", required=True),
    FieldDescriptor("session_id", "str", "Browser/desktop session ID", required=True),
    FieldDescriptor("run_id", "str", "Unique run identifier", required=True),
    FieldDescriptor("target", "str", "CSS selector, URL, or element ref", required=False, default=None),
    FieldDescriptor("goal", "str", "High-level goal for execute action", required=False, default=None),
    FieldDescriptor("parameters", "dict", "Action-specific parameters (x, y, text, keys, ...)", required=False, default={}),
    FieldDescriptor("adapter_preference", "str", "Force a specific adapter: playwright|cdp|desktop", required=False, default=None),
]

_EXECUTE_RESPONSE_FIELDS = [
    FieldDescriptor("run_id", "str", "Run identifier echoed back", required=True),
    FieldDescriptor("session_id", "str", "Session identifier echoed back", required=True),
    FieldDescriptor("adapter_id", "str", "Adapter that handled the action", required=True),
    FieldDescriptor("family", "str", "Action family: browser|desktop|retrieval|hybrid", required=True),
    FieldDescriptor("mode", "str", "Execution mode: execute|inspect|assist", required=True),
    FieldDescriptor("status", "str", "Result status: completed|failed|pending", required=True),
    FieldDescriptor("summary", "str", "Human-readable summary of what happened", required=False),
    FieldDescriptor("extracted_content", "any", "Extracted data (for inspect/extract actions)", required=False),
    FieldDescriptor("artifacts", "list[Artifact]", "Generated artifacts (screenshots, downloads)", required=False),
    FieldDescriptor("receipts", "list[Receipt]", "Audit receipts for each sub-action", required=False),
    FieldDescriptor("error", "ErrorDetail", "Error details if status=failed", required=False),
    FieldDescriptor("trace_id", "str", "Distributed trace ID", required=False),
]


# ---------------------------------------------------------------------------
# Route Registry
# ---------------------------------------------------------------------------

ROUTE_REGISTRY: List[RouteDescriptor] = [

    # ------------------------------------------------------------------
    # System routes
    # ------------------------------------------------------------------
    RouteDescriptor(
        route_id="health",
        method="GET",
        path="/health",
        description="Health check — returns gateway status and session statistics.",
        request_fields=[],
        response_fields=[
            FieldDescriptor("status", "str", "'ok' when healthy", required=True),
            FieldDescriptor("version", "str", "Gateway version string", required=True),
            FieldDescriptor("playwright", "str", "Playwright availability", required=True),
            FieldDescriptor("sessions", "dict", "active/max session counts", required=True),
        ],
        tags=["system"],
    ),
    RouteDescriptor(
        route_id="routes",
        method="GET",
        path="/v1/routes",
        description="Self-documenting route registry — lists every endpoint with field descriptors.",
        request_fields=[],
        response_fields=[
            FieldDescriptor("routes", "list[RouteDescriptor]", "All registered routes", required=True),
            FieldDescriptor("count", "int", "Total number of routes", required=True),
        ],
        tags=["system"],
    ),

    # ------------------------------------------------------------------
    # Discovery routes
    # ------------------------------------------------------------------
    RouteDescriptor(
        route_id="list_apps",
        method="GET",
        path="/v1/apps",
        description="List running macOS applications visible to the accessibility layer.",
        request_fields=[],
        response_fields=[
            FieldDescriptor("apps", "list[dict]", "Running app objects with name, bundle_id, pid", required=True),
            FieldDescriptor("count", "int", "Number of running apps", required=True),
        ],
        tags=["discovery"],
    ),
    RouteDescriptor(
        route_id="list_windows",
        method="GET",
        path="/v1/windows",
        description="List open windows, optionally filtered by app name.",
        request_fields=[
            FieldDescriptor("app_name", "str", "Filter by application name (query param)", required=False, default=None),
        ],
        response_fields=[
            FieldDescriptor("windows", "list[dict]", "Window objects with id, title, bounds, app", required=True),
            FieldDescriptor("count", "int", "Number of windows", required=True),
        ],
        tags=["discovery"],
    ),
    RouteDescriptor(
        route_id="get_window_state",
        method="GET",
        path="/v1/window-state",
        description="Get AX tree + screenshot + coordinate contract for a window. "
                    "Provides full machine-readable context for the current foreground window.",
        request_fields=[
            FieldDescriptor("window_id", "int", "Window ID to inspect (query param)", required=False, default=None),
            FieldDescriptor("skeleton", "bool", "Return compact skeleton tree instead of full AX tree", required=False, default=False),
        ],
        response_fields=[
            FieldDescriptor("window_id", "int", "Resolved window ID", required=True),
            FieldDescriptor("title", "str", "Window title", required=True),
            FieldDescriptor("bounds", "dict", "Window bounds {x, y, width, height}", required=True),
            FieldDescriptor("ax_tree", "dict", "Accessibility tree for the window", required=False),
            FieldDescriptor("screenshot_b64", "str", "Base64 PNG screenshot of the window", required=False),
            FieldDescriptor("coordinate_contract", "dict", "Coordinate system contract for safe clicks", required=False),
        ],
        tags=["discovery"],
    ),
    RouteDescriptor(
        route_id="list_notifications",
        method="GET",
        path="/v1/notifications",
        description="List notification center items visible in the macOS Notification Center.",
        request_fields=[],
        response_fields=[
            FieldDescriptor("notifications", "list[dict]", "Notification objects with id, app, title, body, actions", required=True),
            FieldDescriptor("count", "int", "Number of notifications", required=True),
        ],
        tags=["discovery"],
    ),

    # ------------------------------------------------------------------
    # Action routes (all POST /v1/execute with different action values)
    # ------------------------------------------------------------------
    RouteDescriptor(
        route_id="execute",
        method="POST",
        path="/v1/execute",
        description="Generic action dispatch — routes to the appropriate handler based on action field.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions"],
    ),
    RouteDescriptor(
        route_id="screenshot",
        method="POST",
        path="/v1/execute",
        description="Capture a screenshot of the current page or desktop window. action=screenshot.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "inspect"],
    ),
    RouteDescriptor(
        route_id="click",
        method="POST",
        path="/v1/execute",
        description="Single click at selector or (x, y) coordinates. action=click.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "input"],
    ),
    RouteDescriptor(
        route_id="double_click",
        method="POST",
        path="/v1/execute",
        description="Double click at selector or coordinates. action=double_click.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "input"],
    ),
    RouteDescriptor(
        route_id="right_click",
        method="POST",
        path="/v1/execute",
        description="Right click (context menu) at selector or coordinates. action=right_click.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "input"],
    ),
    RouteDescriptor(
        route_id="fill",
        method="POST",
        path="/v1/execute",
        description="Clear and fill an input field. parameters.text is required. action=fill.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "input"],
    ),
    RouteDescriptor(
        route_id="type",
        method="POST",
        path="/v1/execute",
        description="Type text character-by-character into the focused element. action=type.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "input"],
    ),
    RouteDescriptor(
        route_id="scroll",
        method="POST",
        path="/v1/execute",
        description="Scroll the page or a scrollable element. action=scroll.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "input"],
    ),
    RouteDescriptor(
        route_id="drag",
        method="POST",
        path="/v1/execute",
        description="Drag from one coordinate to another. parameters: from_x, from_y, to_x, to_y, duration_ms. action=drag.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "input"],
    ),
    RouteDescriptor(
        route_id="key",
        method="POST",
        path="/v1/execute",
        description="Press a single key by name (e.g. 'Enter', 'Escape', 'Tab'). action=key.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "input"],
    ),
    RouteDescriptor(
        route_id="key_combo",
        method="POST",
        path="/v1/execute",
        description="Press a key combination (e.g. 'cmd+s', 'ctrl+shift+t'). target contains the combo. action=key_combo.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "input"],
    ),
    RouteDescriptor(
        route_id="set_value",
        method="POST",
        path="/v1/execute",
        description="Set value of an element by AX ref or selector via the accessibility API. action=set_value.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "accessibility"],
    ),
    RouteDescriptor(
        route_id="toggle",
        method="POST",
        path="/v1/execute",
        description="Toggle a checkbox, switch, or radio button element. action=toggle.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "accessibility"],
    ),
    RouteDescriptor(
        route_id="expand",
        method="POST",
        path="/v1/execute",
        description="Expand a collapsible element such as a tree node or dropdown. action=expand.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "accessibility"],
    ),
    RouteDescriptor(
        route_id="collapse",
        method="POST",
        path="/v1/execute",
        description="Collapse an expanded element. action=collapse.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "accessibility"],
    ),
    RouteDescriptor(
        route_id="hover",
        method="POST",
        path="/v1/execute",
        description="Hover over an element or coordinates to reveal tooltips/menus. action=hover.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "input"],
    ),
    RouteDescriptor(
        route_id="triple_click",
        method="POST",
        path="/v1/execute",
        description="Triple click to select all text in a field. action=triple_click.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "input"],
    ),
    RouteDescriptor(
        route_id="get_clipboard",
        method="POST",
        path="/v1/execute",
        description="Read the current clipboard contents. action=get_clipboard.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "system"],
    ),
    RouteDescriptor(
        route_id="set_clipboard",
        method="POST",
        path="/v1/execute",
        description="Write text to the clipboard. target contains the text. action=set_clipboard.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "system"],
    ),
    RouteDescriptor(
        route_id="find_elements",
        method="POST",
        path="/v1/execute",
        description="Find elements matching a query in the AX tree. target is the query string. action=find_elements.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "accessibility"],
    ),
    RouteDescriptor(
        route_id="goto",
        method="POST",
        path="/v1/execute",
        description="Navigate a browser session to a URL. action=goto.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "browser"],
    ),
    RouteDescriptor(
        route_id="extract",
        method="POST",
        path="/v1/execute",
        description="Extract text/HTML/JSON content from the current page. action=extract.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "browser"],
    ),
    RouteDescriptor(
        route_id="inspect",
        method="POST",
        path="/v1/execute",
        description="Inspect current page structure or specific element. action=inspect.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "inspect"],
    ),
    RouteDescriptor(
        route_id="ax_snapshot",
        method="POST",
        path="/v1/execute",
        description="Get a full accessibility tree snapshot for the current app. action=ax_snapshot.",
        request_fields=_EXECUTE_REQUEST_FIELDS,
        response_fields=_EXECUTE_RESPONSE_FIELDS,
        tags=["actions", "accessibility"],
    ),

    # ------------------------------------------------------------------
    # App / window management
    # ------------------------------------------------------------------
    RouteDescriptor(
        route_id="launch_app",
        method="POST",
        path="/v1/apps/launch",
        description="Launch a macOS application by name or bundle ID.",
        request_fields=[
            FieldDescriptor("name", "str", "Application display name", required=True),
            FieldDescriptor("bundle_id", "str", "Bundle identifier (e.g. com.apple.Safari)", required=False, default=None),
        ],
        response_fields=[
            FieldDescriptor("success", "bool", "Whether the app launched successfully", required=True),
            FieldDescriptor("pid", "int", "Process ID of the launched app", required=False),
            FieldDescriptor("error", "str", "Error message if launch failed", required=False),
        ],
        tags=["app-management"],
    ),
    RouteDescriptor(
        route_id="close_app",
        method="POST",
        path="/v1/apps/close",
        description="Quit a running application gracefully.",
        request_fields=[
            FieldDescriptor("name", "str", "Application display name to quit", required=True),
        ],
        response_fields=[
            FieldDescriptor("success", "bool", "Whether the app was closed", required=True),
            FieldDescriptor("error", "str", "Error message if close failed", required=False),
        ],
        tags=["app-management"],
    ),
    RouteDescriptor(
        route_id="focus_window",
        method="POST",
        path="/v1/windows/focus",
        description="Bring a window to the foreground.",
        request_fields=[
            FieldDescriptor("window_id", "int", "Window ID to focus", required=False, default=None),
            FieldDescriptor("title", "str", "Window title to match", required=False, default=None),
            FieldDescriptor("app_name", "str", "Application name to focus", required=False, default=None),
        ],
        response_fields=[
            FieldDescriptor("success", "bool", "Whether focus was applied", required=True),
            FieldDescriptor("window_id", "int", "Window that was focused", required=False),
            FieldDescriptor("error", "str", "Error message if focus failed", required=False),
        ],
        tags=["window-management"],
    ),
    RouteDescriptor(
        route_id="resize_window",
        method="POST",
        path="/v1/windows/resize",
        description="Resize a window to new dimensions or by dragging an edge.",
        request_fields=[
            FieldDescriptor("window_id", "int", "Window ID to resize", required=True),
            FieldDescriptor("x", "float", "New x position", required=False, default=None),
            FieldDescriptor("y", "float", "New y position", required=False, default=None),
            FieldDescriptor("width", "float", "New width", required=False, default=None),
            FieldDescriptor("height", "float", "New height", required=False, default=None),
            FieldDescriptor("edge", "str", "Edge to resize from: top|bottom|left|right", required=False, default=None),
            FieldDescriptor("delta", "float", "Pixels to resize by when using edge mode", required=False, default=None),
        ],
        response_fields=[
            FieldDescriptor("success", "bool", "Whether resize succeeded", required=True),
            FieldDescriptor("bounds", "dict", "New window bounds {x, y, width, height}", required=False),
            FieldDescriptor("error", "str", "Error message if resize failed", required=False),
        ],
        tags=["window-management"],
    ),
    RouteDescriptor(
        route_id="drag_window",
        method="POST",
        path="/v1/windows/drag",
        description="Move a window by dragging its title bar by a delta offset.",
        request_fields=[
            FieldDescriptor("window_id", "int", "Window ID to move", required=True),
            FieldDescriptor("delta_x", "float", "Horizontal drag delta in points", required=True),
            FieldDescriptor("delta_y", "float", "Vertical drag delta in points", required=True),
        ],
        response_fields=[
            FieldDescriptor("success", "bool", "Whether the drag succeeded", required=True),
            FieldDescriptor("bounds", "dict", "New window bounds after drag", required=False),
            FieldDescriptor("error", "str", "Error message if drag failed", required=False),
        ],
        tags=["window-management"],
    ),

    # ------------------------------------------------------------------
    # Notification actions
    # ------------------------------------------------------------------
    RouteDescriptor(
        route_id="dismiss_notification",
        method="POST",
        path="/v1/notifications/{notification_id}/dismiss",
        description="Dismiss a notification from the Notification Center.",
        request_fields=[
            FieldDescriptor("notification_id", "str", "Notification ID (path parameter)", required=True),
        ],
        response_fields=[
            FieldDescriptor("success", "bool", "Whether dismiss succeeded", required=True),
            FieldDescriptor("error", "str", "Error message if dismiss failed", required=False),
        ],
        tags=["notifications"],
    ),
    RouteDescriptor(
        route_id="notification_action",
        method="POST",
        path="/v1/notifications/{notification_id}/action",
        description="Perform a notification action such as 'Archive', 'Reply', or 'Mark as Read'.",
        request_fields=[
            FieldDescriptor("notification_id", "str", "Notification ID (path parameter)", required=True),
            FieldDescriptor("action", "str", "Action name to trigger (e.g. 'Archive', 'Reply')", required=True),
        ],
        response_fields=[
            FieldDescriptor("success", "bool", "Whether the action was performed", required=True),
            FieldDescriptor("error", "str", "Error message if action failed", required=False),
        ],
        tags=["notifications"],
    ),

    # ------------------------------------------------------------------
    # Planning / execution routes
    # ------------------------------------------------------------------
    RouteDescriptor(
        route_id="computer_use_execute",
        method="POST",
        path="/v1/computer-use/execute",
        description="Submit a natural-language task for autonomous computer use execution.",
        request_fields=[
            FieldDescriptor("task", "str", "Natural language task description", required=True),
            FieldDescriptor("session_id", "str", "Session ID", required=False, default=None),
            FieldDescriptor("run_id", "str", "Override run ID", required=False, default=None),
            FieldDescriptor("mode", "str", "Execution mode", required=False, default="execute"),
            FieldDescriptor("max_steps", "int", "Maximum planning loop steps", required=False, default=20),
            FieldDescriptor("approval_policy", "str", "Approval policy: never|on-risk|always", required=False, default="on-risk"),
        ],
        response_fields=[
            FieldDescriptor("run_id", "str", "Run identifier for polling / SSE", required=True),
            FieldDescriptor("status", "str", "Initial status", required=True),
        ],
        tags=["planning"],
    ),
    RouteDescriptor(
        route_id="computer_use_run",
        method="GET",
        path="/v1/computer-use/runs/{run_id}",
        description="Poll the status and result of a computer use run.",
        request_fields=[
            FieldDescriptor("run_id", "str", "Run ID (path parameter)", required=True),
        ],
        response_fields=[
            FieldDescriptor("run_id", "str", "Run identifier", required=True),
            FieldDescriptor("status", "str", "Current status", required=True),
            FieldDescriptor("steps", "list[dict]", "Completed steps so far", required=False),
            FieldDescriptor("result", "dict", "Final result once completed", required=False),
        ],
        tags=["planning"],
    ),
    RouteDescriptor(
        route_id="computer_use_steer",
        method="POST",
        path="/v1/computer-use/runs/{run_id}/steer",
        description="Send the user's guidance to a running planning loop; it reads it before its next step.",
        request_fields=[
            FieldDescriptor("run_id", "str", "Run ID (path parameter)", required=True),
            FieldDescriptor("text", "str", "The guidance", required=True),
        ],
        response_fields=[
            FieldDescriptor("accepted", "bool", "Whether the run took it", required=True),
        ],
        tags=["planning"],
    ),
    RouteDescriptor(
        route_id="computer_use_approve",
        method="POST",
        path="/v1/computer-use/runs/{run_id}/approve",
        description="Approve a pending action that requires human approval.",
        request_fields=[
            FieldDescriptor("run_id", "str", "Run ID (path parameter)", required=True),
        ],
        response_fields=[
            FieldDescriptor("success", "bool", "Whether approval was accepted", required=True),
        ],
        tags=["planning"],
    ),
    RouteDescriptor(
        route_id="computer_use_cancel",
        method="POST",
        path="/v1/computer-use/runs/{run_id}/cancel",
        description="Cancel a running computer use task.",
        request_fields=[
            FieldDescriptor("run_id", "str", "Run ID (path parameter)", required=True),
        ],
        response_fields=[
            FieldDescriptor("success", "bool", "Whether cancellation was accepted", required=True),
        ],
        tags=["planning"],
    ),
]
