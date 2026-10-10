# GP-07: Cross-Family Browser + Desktop

## Purpose
Use browser automation and desktop automation together in the same
workflow — e.g., download a file in the browser, then open it in
a native desktop app.

## Preconditions
- Chromium installed (for browser)
- Desktop visible with the Accessibility permission (macOS)
- Both adapters available

## Routing
Two separate sessions — one per family. The routing system enforces
session isolation (G2) so browser and desktop contexts don't leak.

```
# Session 1: browser
Router.route(family="browser", mode="execute")  → browser.playwright

# Session 2: desktop
Router.route(family="desktop", mode="desktop")  → desktop.accessibility
```

## Execution Flow
```
# Phase A — Browser
SessionManager.create(family="browser")
PlaywrightAdapter.execute(goto, url)
PlaywrightAdapter.execute(act, "click download")
PlaywrightAdapter.execute(screenshot)

# Phase B — Desktop
SessionManager.create(family="desktop")
AccessibilityAdapter.execute("take_screenshot")      # verify file appeared
AccessibilityAdapter.execute("click", {x, y})        # open the file
AccessibilityAdapter.execute("take_screenshot")      # verify app opened

# Phase C — Isolation
assert browser_session.artifact_root != desktop_session.artifact_root
assert cross_session_policy.decision == "deny"

# Cleanup
SessionManager.destroy(browser_session)
SessionManager.destroy(desktop_session)
```

## Evidence Requirements
- Per-session screenshots
- Artifact roots verified separate

## Receipt Requirements
- Route decision receipts for both sessions
- Per-action receipts in each family
- Cross-session access denied receipt (G2)

## Conformance
- Suite A (browser portion)
- Suite D (desktop portion)
- Suite F: F-06 session isolation
