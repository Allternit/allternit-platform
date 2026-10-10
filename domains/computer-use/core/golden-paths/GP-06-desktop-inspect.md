# GP-06: Desktop Inspect (accessibility adapter — Read-Only)

## Purpose
Capture desktop state for debugging — screenshot the screen, read mouse
position, observe screen dimensions. No clicks or typing.

## Preconditions
- Desktop visible
- macOS with the Screen Recording permission granted

## Routing
- **Family:** desktop
- **Mode:** inspect
- **Primary adapter:** desktop.accessibility
- **Fail mode:** fail open (read-only)

## Execution Flow
```
goal → Router.route(family="desktop", mode="inspect")
     → PolicyEngine.evaluate(action_type="observe")  # read-only
     → SessionManager.create(family="desktop")
     → AccessibilityAdapter.execute("take_screenshot")
     → AccessibilityAdapter.screen_size() / cursor_position()
     → ReceiptWriter.emit(...)
     → SessionManager.destroy()
```

## Evidence Requirements
- Full screen screenshot
- Screen size and mouse position

## Receipt Requirements
- Route decision receipt
- Observe/screenshot receipts (lower evidence bar — read-only)

## Conformance
- Suite D: D-01 screenshot, D-02 observe
