# Cookbook: Triage GitHub Issue

## Goal
Read a GitHub issue, determine its category from the title and body, apply an appropriate
label, and post a standardised triage comment acknowledging receipt and summarising next steps.

## Preconditions
- The agent is authenticated to GitHub.
- The issue URL is known and the issue is in `open` state.
- Labels exist on the repository (the cookbook does not create new labels).
- Operator approval is configured for label application and comment posting.

## Family
browser

## Mode
execute

## Primary Adapter
browser.playwright

## Fallback
planning loop (vision model driving browser.playwright)

## Policy
- Plugin policy profile: `github` (max_destructive_actions: 3, requires_approval: true)
- Applying a label and posting a comment are each classified as **destructive actions**.
- Both require operator approval when `requires_approval: true`.
- Navigation within github.com is unrestricted.
- `close_all_issues` is blocked unconditionally.

## Steps

1. **Navigate to issue**
   - `goto(url=<ISSUE_URL>)`
   - Assert page title contains the issue number.
   - Screenshot: `before-issue-page.png`

2. **Extract issue content**
   - `extract(selector="h1.gh-header-title span.js-issue-title")` → issue title
   - `extract(selector="div.comment-body p", multiple=true, limit=10)` → body paragraphs
   - `extract(selector="span.css-truncate-target[data-menu-button]", multiple=true)` → existing labels
   - `extract(selector="a.author")` → reporter username
   - Store: `{ issue_title, issue_body, existing_labels, reporter }`

3. **Classify issue** (LLM step)
   - Use embedded classification logic with fallback to the planning loop:
     - Categories: `bug`, `enhancement`, `question`, `documentation`, `duplicate`, `wontfix`
   - Select the label that best matches the issue content.
   - If `existing_labels` already contains the target label, skip step 4.
   - Store: `{ chosen_label, classification_reasoning }`

4. **Apply label** (destructive action — requires approval)
   - Emit `ApprovalRequest { action: "apply_label", label: chosen_label, issue_url }`
   - On approval:
     - `click(selector="details-menu[src*='/labels'] summary")` to open label picker
     - Wait for `details-menu[src*='/labels'] .select-menu-item` list to appear.
     - `click(selector=f"//div[@role='menuitem'][.//span[text()='{chosen_label}']]")` to select
     - `click` outside the menu to close it.
     - Screenshot: `label-applied.png`
   - On denial: skip label, record `label_status: skipped`.

5. **Compose triage comment** (LLM step)
   - Variables: `{{ issue_title }}`, `{{ issue_body }}`, `{{ chosen_label }}`,
     `{{ classification_reasoning }}`, `{{ reporter }}`
   - Output: `triage_comment` (string, max 500 chars)
   - Template structure:
     ```
     Thanks for opening this issue, @{{ reporter }}.
     This has been triaged as **{{ chosen_label }}**.
     {{ classification_reasoning }}
     We'll follow up once the team has reviewed.
     ```

6. **Post triage comment** (destructive action — requires approval)
   - Emit `ApprovalRequest { action: "post_triage_comment", payload: triage_comment }`
   - On approval:
     - `click(selector="textarea#new_comment_field")` to focus comment box
     - `type(selector="textarea#new_comment_field", text=triage_comment)`
     - Screenshot: `comment-form-filled.png`
     - `click(selector="button.btn-primary[type='submit']")` to submit
   - Wait for the new comment timeline item to appear.
   - Screenshot: `comment-posted.png`

7. **Verify outcome**
   - `extract(selector="div.timeline-comment-group:last-child div.comment-body p", first=true)`
   - Assert extracted text contains key phrase from `triage_comment`.

8. **Emit receipt**
   - `ReceiptWriter.emit({ action: "triage_issue", issue_url, chosen_label, triage_comment, status: "success" })`

## Expected Artifacts
- `before-issue-page.png` — screenshot of issue page on arrival
- `label-applied.png` — screenshot showing the new label in the sidebar
- `comment-form-filled.png` — screenshot of comment textarea before submit
- `comment-posted.png` — screenshot confirming the posted comment
- `receipt.json` — receipt with integrity hash, issue_url, chosen_label, triage_comment excerpt

## Failure Handling
- **Issue is closed**: emit receipt `status: skipped`, `reason: issue_already_closed`. Abort.
- **Auth expired**: emit receipt `status: error`, `reason: auth_required`. Abort.
- **Label not found in picker**: fall back to the planning loop (vision model driving `browser.playwright`) to locate label by visual scan. If still not found, emit receipt `status: partial`, `reason: label_not_found`, continue to step 5.
- **Either approval denied**: record `status: partial` on receipt with appropriate `reason`.
- **Both approvals denied**: emit receipt `status: blocked`. No changes made to the issue.
- **Comment submission timeout**: screenshot `timeout-state.png`, abort with `status: error`, `reason: timeout`.

## Conformance
Suite: `plugin-github-v1`
- PG-07: navigate to issue and extract metadata
- PG-08: classify issue and select label
- PG-09: apply label with approval gate
- PG-10: post triage comment with approval gate
- PG-11: receipt emitted with integrity hash
- PG-12: partial receipt when one action is denied
