# GP-05: Desktop Execute (accessibility adapter)

## Purpose
Automate native desktop applications — screenshot, click, type, observe.
For legacy apps, GUI-only systems, or cross-app workflows.

## Preconditions
- Target application launchable
- Desktop visible (not headless server)
- macOS with the Accessibility (and Screen Recording) permission granted
- Screen accessible (not locked)

## Routing
- **Family:** desktop
- **Mode:** desktop (or execute — both route to desktop.accessibility)
- **Primary adapter:** desktop.accessibility
- **Fallback chain:** none
- **Fail mode:** fail closed

## Execution Flow
```
goal → Router.route(family="desktop", mode="desktop")
     → PolicyEngine.evaluate(adapter_risk_level="high", family="desktop")
       → forces headed mode (P-003)
     → SessionManager.create(family="desktop")
     → AccessibilityAdapter.execute("take_screenshot")          # full screen capture
     → AccessibilityAdapter.screen_size() / cursor_position()   # observe
     → AccessibilityAdapter.execute("click", {x: 500, y: 300})
     → AccessibilityAdapter.execute("type_text", {text: "hello"})
     → ReceiptWriter.emit(...)
     → SessionManager.destroy()
```

## Supported Actions
| Action       | Accessibility adapter call            | Notes                    |
|-------------|----------------------------------------|--------------------------|
| `screenshot` | `execute("take_screenshot")`          | Full screen PNG          |
| `observe`    | `screen_size()`, `cursor_position()`  | Screen dims + cursor     |
| `act:click`  | `execute("click", {x, y})`            | Quartz event, absolute   |
| `act:type`   | `execute("type_text", {text})`        | Quartz keyboard events   |

## Evidence Requirements
- Screenshot before each action
- Screenshot after each action
- Mouse position logged

## Receipt Requirements
- Route decision receipt (G5)
- Receipt per desktop action with integrity hash (G3)
- Policy forces headed mode receipt

## Conformance
- Suite D: D-01 screenshot, D-02 observe, D-03 envelope, D-04 receipt
