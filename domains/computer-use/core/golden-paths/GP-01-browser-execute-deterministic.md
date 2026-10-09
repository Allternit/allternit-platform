# GP-01: Deterministic Browser Execute (Playwright)

## Purpose
Automate a stable web workflow — navigate, fill forms, click, extract, screenshot.
Selectors are known and stable. No LLM reasoning needed.

## Preconditions
- Target URL reachable
- Selectors/flow documented
- Chromium installed (`playwright install chromium`)

## Routing
- **Family:** browser
- **Mode:** execute
- **Constraints:** `deterministic=True`
- **Primary adapter:** browser.playwright
- **Fallback chain:** none
- **Fail mode:** fail closed

## Execution Flow
```
goal → Router.route(family="browser", mode="execute", deterministic=True)
     → PolicyEngine.evaluate(target=url, action_type="goto")
     → SessionManager.create(family="browser")
     → PlaywrightAdapter.execute(goto, url)
     → PlaywrightAdapter.execute(extract, selector)
     → PlaywrightAdapter.execute(screenshot, page)
     → ReceiptWriter.emit(action_data, result_data, integrity_hash)
     → SessionManager.destroy()
```

## Evidence Requirements
- Screenshot before and after each action
- DOM hash at each checkpoint
- Page title and URL captured in result envelope

## Receipt Requirements
- Route decision receipt (G5)
- Receipt per action with integrity hash (G3)
- Policy decision logged

## Conformance
- Suite A tests: A-01 navigate, A-02 extract, A-03 screenshot, A-04 eval, A-05 HTML, A-07 observe, A-08 envelope
