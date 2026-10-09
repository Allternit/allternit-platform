# Desktop Automation Adapters

Native desktop automation for `allternit-computer-use`.

- **`accessibility_adapter.py`** (`desktop.accessibility`): the desktop adapter.
  Input goes through Quartz CGEvents (`core/background_events.py`, optionally
  delivered per-process via SkyLight), element discovery and AX actions go
  through the macOS accessibility tree, and window/app management uses
  AppleScript. Screenshots use `screencapture`.
- **`hybrid_adapter.py`**: combines the accessibility adapter with a browser
  adapter for cross-family steps.

The PyAutoGUI and Operator adapters were removed in the D0 cleanup; there is no
foreground pyautogui/pynput fallback any more. Without Quartz (non-macOS hosts),
native input actions report failure instead of silently doing nothing.

## Installation

```bash
pip install -e '.[desktop-macos]'   # pyobjc Quartz + ApplicationServices
```

## Permission Setup (macOS)

Grant **Accessibility** permissions (required for input and AX actions):

1. Open **System Settings** → **Privacy & Security** → **Accessibility**
2. Add the app that runs the gateway (Allternit Desktop, or your terminal /
   Python interpreter when running it directly)
3. Ensure the checkbox is checked

Grant **Screen Recording** permissions (for screenshots):

1. Open **System Settings** → **Privacy & Security** → **Screen Recording**
2. Add the same app
3. Restart that app after granting permissions

## Usage

```python
from adapters.desktop.accessibility_adapter import AccessibilityAdapter

adapter = AccessibilityAdapter()
await adapter.execute("click", {"x": 500, "y": 300})
await adapter.execute("type_text", {"text": "hello"})
await adapter.execute("key_combo", {"combo": "cmd+s"})
shot = await adapter.execute("take_screenshot", {})   # {"success", "image_b64"}
adapter.cursor_position()                              # (x, y) or None
adapter.screen_size()                                  # (w, h) or None
```

`execute(action, params)` returns `{"success": bool, ...}`. The command set
covers mouse (`click`, `double_click`, `right_click`, `triple_click`, `hover`,
`drag`, `mouse_down`, `mouse_up`), keyboard (`type_text`, `press_key`,
`key_combo`, `key_down`, `key_up`), scrolling, AX state and element discovery,
app and window management, clipboard, screenshots and notifications.

The gateway registers this adapter as `desktop.accessibility` at startup and
uses it for `/v1/execute` desktop actions that arrive without an active
browser session.
