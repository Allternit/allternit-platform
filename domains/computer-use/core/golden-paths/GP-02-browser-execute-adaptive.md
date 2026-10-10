# GP-02: Adaptive Browser Execute (planning loop + Playwright)

## Purpose
Extract data or complete tasks on websites with shifting UIs.
A vision model reasons about each screenshot and adapts to DOM changes;
the steps it chooses run through the Playwright adapter.

## Preconditions
- Target URL reachable
- Chromium installed for Playwright
- A configured vision/planning model (see `core/vision_providers.py`)

## Routing
- **Family:** browser
- **Mode:** execute
- **Constraints:** `deterministic=False`
- **Primary adapter:** browser.playwright
- **Fallback chain:** none
- **Fail mode:** fail closed

## Execution Flow
```
goal → Router.route(family="browser", mode="execute", deterministic=False)
     → PolicyEngine.evaluate(target=url, action_type="goto")
     → SessionManager.create(family="browser")
     → PlanningLoop.run(goal)            # screenshot → model → next action
       → PlaywrightAdapter.execute(...)  # one call per chosen step
     → ReceiptWriter.emit(action_data, result_data, integrity_hash)
     → SessionManager.destroy()
```

## Evidence Requirements
- Screenshot before extraction
- Extracted content captured as artifact
- Planning log (what steps the model took)

## Receipt Requirements
- Route decision receipt (G5)
- Action receipt per model-driven step (G3)
- Content hash on extracted data

## Conformance
- Suite A tests apply to the Playwright adapter that executes the steps
