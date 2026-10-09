# GP-10: Plugin End-to-End Workflow

## Purpose
Verify that a domain plugin can be loaded from the registry, its cookbook executed
against a real (or stubbed) target, conformance validation passes, and the cookbook's
described behaviour matches what actually occurred during the run.

This golden path exercises the **full vertical slice** from plugin discovery through
recipe execution to artifact validation — it is the primary integration test for the
plugin system.

## Preconditions
- `PluginRegistry` is initialised with the `plugins/` directory.
- The target plugin (`github` in this path) is present in `plugins/github/` with a
  valid `plugin.json`.
- `browser.playwright` adapter is available (Chromium installed).
- A test GitHub pull request URL is available:
  `https://github.com/allternit/computer-use/pull/1` (or a localhost HTML stub
  mimicking a GitHub PR page structure).
- The agent is authenticated to GitHub (session cookie or stub bypass active).
- Policy engine is running with the `github` plugin profile loaded.
- Operator approval is pre-granted for the test run (`auto_approve=True` in test config).

## Routing
- **Family:** browser
- **Mode:** execute
- **Plugin:** github
- **Cookbook:** review-pr
- **Primary adapter:** browser.playwright
- **Fallback chain:** none
- **Fail mode:** fail closed

## Execution Flow

```
# Phase 1: Plugin Loading
registry = PluginRegistry(plugins_dir="packages/computer-use/plugins")
manifests = registry.discover()
  → scans plugins/ directory
  → finds plugins/github/plugin.json
  → validates manifest against plugin.manifest.schema.json
  → indexes under id="github"

plugin = registry.get("github")
  → returns PluginManifest(id="github", version="0.1.0", ...)

assert plugin is not None
assert plugin.production_status in ("beta", "production")
assert "GP-10" in plugin.golden_paths
assert "review-pr" in plugin.cookbooks

loader = PluginLoader()
cookbook_text = loader.load_cookbook(plugin, "review-pr")
  → reads plugins/github/cookbooks/review-pr.md
  → returns non-empty string

prompt_text = loader.load_prompt(plugin, "review-pr")
  → reads plugins/github/prompts/review-pr.txt
  → returns non-empty string

# Phase 2: Policy Profile Application
PolicyEngine.load_plugin_profile(plugin.policy_profile)
  → registers allowed_domains: ["github.com", "*.github.com"]
  → registers blocked_actions: ["delete_repository", "force_push", ...]
  → sets max_destructive_actions: 3
  → sets requires_approval: True (bypassed in test by auto_approve=True)

# Phase 3: Cookbook Execution
Router.route(
    family="browser",
    mode="execute",
    plugin_id="github",
    cookbook_id="review-pr",
    target_url="https://github.com/allternit/computer-use/pull/1"
)
  → PolicyEngine.evaluate(target=target_url, action_type="goto")
      → decision: allow (github.com in allowed_domains)
  → SessionManager.create(family="browser", session_id="gp10-test")
  → PlaywrightAdapter.execute(goto, target_url)
  → PlaywrightAdapter.execute(extract, "span.js-issue-title")       → pr_title
  → PlaywrightAdapter.execute(click, "a[data-tab-item='files-tab']")
  → PlaywrightAdapter.execute(extract, "div.file-header[data-path]", multiple=True)
  → LLM.invoke(prompt_text, vars={pr_title, pr_author, changed_files, ...})
      → review_text
  → PolicyEngine.evaluate(action_type="post_review_comment")
      → decision: require_approval → auto_approve=True → allow
  → PlaywrightAdapter.execute(click, "button[data-hotkey='p']")
  → PlaywrightAdapter.execute(type, "textarea#pull_request_review_body", review_text)
  → PlaywrightAdapter.execute(click, "button.btn-primary[type='submit']")
  → ReceiptWriter.emit(action="post_review_comment", pr_url, review_text, status="success")
  → SessionManager.destroy(session_id="gp10-test")

# Phase 4: Conformance Validation
ConformanceSuite.run(suite_id="plugin-github-v1", session_artifacts=artifacts)
  → PG-01 pass: PR page navigated
  → PG-02 pass: Files Changed tab opened and diff extracted
  → PG-03 pass: review composed via prompt
  → PG-04 pass: review comment posted and submission confirmed
  → PG-05 pass: receipt present with integrity hash
  → PG-06 pass: policy approval gate recorded in receipt

# Phase 5: Cookbook Behaviour Verification
CookbookVerifier.verify(cookbook_text, session_artifacts=artifacts)
  → checks each declared Step has a corresponding artifact or log entry
  → checks Expected Artifacts section lists match actual files present
  → verifies policy section claims match PolicyEngine decision log
```

## Verification Assertions

### Plugin Loading Assertions
| Assertion | Expected |
|---|---|
| `registry.get("github")` is not None | True |
| `plugin.id` | `"github"` |
| `plugin.version` | valid semver |
| `plugin.families` | contains `"browser"` |
| `plugin.modes` | contains `"execute"` |
| `plugin.cookbooks` | contains `"review-pr"` |
| `plugin.golden_paths` | contains `"GP-10"` |
| `plugin.conformance_suite` | `"plugin-github-v1"` |
| `plugin.policy_profile["requires_approval"]` | `True` |
| `loader.load_cookbook(plugin, "review-pr")` | non-empty string |
| `loader.load_prompt(plugin, "review-pr")` | non-empty string |
| manifest validation errors | `[]` (no errors) |

### Cookbook Execution Assertions
| Assertion | Expected |
|---|---|
| `before-pr-page.png` artifact present | True |
| `diff-view.png` artifact present | True |
| `review-form-filled.png` artifact present | True |
| `review-submitted.png` artifact present | True |
| `receipt.json` present | True |
| receipt `status` | `"success"` |
| receipt `action` | `"post_review_comment"` |
| receipt `integrity_hash` | non-empty string |
| policy decision log contains approval event | True |
| review_text length | 1 ≤ len ≤ 2000 characters |

### Conformance Suite Assertions
| Suite Test | Expected Outcome |
|---|---|
| PG-01: navigate to PR | PASS |
| PG-02: extract diff | PASS |
| PG-03: compose review via prompt | PASS |
| PG-04: post review comment | PASS |
| PG-05: receipt with integrity hash | PASS |
| PG-06: policy approval gate fires | PASS |

### Cookbook Behaviour Match Assertions
| Claim in Cookbook | Verified By |
|---|---|
| "Primary Adapter: browser.playwright" | session log adapter_id = "browser.playwright" |
| "Policy: requires_approval: true" | PolicyEngine decision log shows approval gate |
| All 9 steps completed | one log/artifact entry per step |
| Expected Artifacts list | all 5 files present in artifact dir |

## Stub / Offline Mode
When a live GitHub session is not available, a local HTML stub can be used:

```python
# conftest.py
@pytest.fixture
def github_pr_stub(httpserver):
    httpserver.expect_request("/pull/1").respond_with_data(
        GITHUB_PR_STUB_HTML,   # defined in tests/stubs/github_pr.html
        content_type="text/html"
    )
    return httpserver.url_for("/pull/1")
```

The stub must render the following selectors to satisfy cookbook steps:
- `span.js-issue-title` (PR title)
- `a[data-tab-item='files-tab']` (Files Changed tab)
- `div.file-header[data-path]` (at least 1 file entry)
- `button[data-hotkey='p']` (Review Changes button)
- `textarea#pull_request_review_body` (review text area)
- `button.btn-primary[type='submit']` (Submit Review button)
- `div.js-timeline-item` (confirmation element post-submit)

## Evidence Requirements
- All plugin loading assertions pass.
- All cookbook execution artifacts are present with non-zero file size.
- Receipt `status` is `"success"` with a valid `integrity_hash`.
- Conformance suite `plugin-github-v1` reports all 6 tests as PASS.
- Cookbook behaviour match: all step-level claims verified.
- Session cleanly destroyed (no leaked browser processes after test).

## Receipt Requirements
- `plugin_id: "github"`
- `cookbook_id: "review-pr"`
- `golden_path: "GP-10"`
- `conformance_suite: "plugin-github-v1"`
- `conformance_result: "pass"`
- `adapter_id: "browser.playwright"`
- `status: "success"`
- `artifacts`: all 5 expected artifact paths with `integrity_hash` per file
- `policy_decisions`: at minimum the approval gate event

## Conformance
Suite P: Plugin End-to-End
- P-01: plugin discovered and loaded from registry without errors
- P-02: manifest validates against plugin.manifest.schema.json with zero errors
- P-03: cookbook file loads and is non-empty
- P-04: prompt file loads and is non-empty
- P-05: policy profile applied to PolicyEngine before execution
- P-06: cookbook executes to completion with status "success"
- P-07: all Expected Artifacts listed in cookbook are present on disk
- P-08: conformance suite plugin-github-v1 passes all declared tests
- P-09: cookbook behaviour match confirms steps align with artifacts
- P-10: session destroyed cleanly; no leaked processes
