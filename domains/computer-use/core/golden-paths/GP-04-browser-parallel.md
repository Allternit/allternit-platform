# GP-04: Parallel Browser Execution (Playwright Pool)

## Purpose
Fan out the same or different tasks across multiple browser contexts.
Price comparison, A/B test capture, multi-page scraping.

## Preconditions
- Chromium installed
- System has enough memory for N contexts (each ~100-200MB)

## Routing
- **Family:** browser
- **Mode:** parallel
- **Primary adapter:** browser.playwright
- **Fallback chain:** none
- **Fail mode:** fail closed (partial results returned for completed contexts)

## Execution Flow
```
goal → Router.route(family="browser", mode="parallel")
     → PolicyEngine.evaluate(...)
     → SessionManager.create(family="browser")  # parent session
     → for each target in targets:
         → PlaywrightAdapter — new browser context
         → execute(goto, url)
         → execute(extract, selector)
         → execute(screenshot)
     → Aggregate results into parent envelope
     → ReceiptWriter.emit per context + aggregate receipt
     → SessionManager.destroy()
```

## Evidence Requirements
- Screenshot per context
- Extracted data per context
- Aggregate result summary

## Receipt Requirements
- Route decision receipt (G5)
- Per-context action receipts (G3)
- Aggregate receipt linking all child runs

## Conformance
- Suite A tests per individual context
- Suite F: parallel routing determinism
