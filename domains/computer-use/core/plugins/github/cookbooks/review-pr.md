# Cookbook: Review Pull Request

## Goal
Open a GitHub pull request, read the diff, post a structured review comment summarising
key findings, and screenshot the submitted review as a receipt artifact.

## Preconditions
- The agent is authenticated to GitHub (session cookie or OAuth token present in browser profile).
- The PR URL is known and accessible (public repo or authenticated private repo).
- The PR is in `open` state.
- The operator has approved the `post_review_comment` action when `requires_approval: true`.

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
- Posting a review comment is classified as a **destructive action** (policy rule P-002).
- Navigation to github.com is unrestricted within the `allowed_domains` list.
- `dismiss_review` and `force_push` are blocked unconditionally.

## Steps

1. **Navigate to PR**
   - `goto(url=<PR_URL>)`
   - Assert page title contains the PR number and repository name.
   - Screenshot: `before-pr-page.png`

2. **Extract PR metadata**
   - `extract(selector="span.js-issue-title")` → PR title
   - `extract(selector="span#partial-discussions-header .f6.color-fg-muted a")` → author
   - `extract(selector="div.tabnav-tabs a[data-tab-item='files-tab'] span.Counter")` → file count
   - Store: `{ pr_title, pr_author, files_changed_count }`

3. **Open Files Changed tab**
   - `click(selector="a[data-tab-item='files-tab']")`
   - Wait for `div.js-diff-progressive-container` to be visible.
   - Screenshot: `diff-view.png`

4. **Extract diff summary**
   - `extract(selector="div#files_bucket .file-header[data-path]", multiple=true)` → list of changed file paths
   - `extract(selector="td.blob-code-addition", multiple=true, limit=50)` → first 50 added lines
   - `extract(selector="td.blob-code-deletion", multiple=true, limit=50)` → first 50 removed lines
   - Store: `{ changed_files: [...], additions_sample: [...], deletions_sample: [...] }`

5. **Compose review comment** (LLM step — planning loop or external call)
   - Prompt: `prompts/review-pr.txt` with variables:
     - `{{ pr_title }}`, `{{ pr_author }}`, `{{ changed_files }}`,
       `{{ additions_sample }}`, `{{ deletions_sample }}`
   - Output: `review_text` (string, max 2000 chars)

6. **Request operator approval** (if `requires_approval: true`)
   - Emit `ApprovalRequest { action: "post_review_comment", payload: review_text }`
   - Wait for `ApprovalResponse { approved: true }` before continuing.
   - If denied → abort, emit receipt with `status: blocked`.

7. **Navigate to review form**
   - `click(selector="button[data-hotkey='p']")` (or "Review changes" button)
   - Wait for `div.pull-request-review-menu` to be visible.

8. **Post review comment**
   - `click(selector="textarea#pull_request_review_body")` to focus
   - `type(selector="textarea#pull_request_review_body", text=review_text)`
   - `click(selector="input[value='COMMENT']")` to select "Comment" review type
   - Screenshot: `review-form-filled.png`
   - `click(selector="button.btn-primary[type='submit']")` to submit

9. **Confirm submission**
   - Wait for `div.js-timeline-item` containing the posted review.
   - `extract(selector="div.review-comment-contents p", first=true)` → verify text matches
   - Screenshot: `review-submitted.png`

10. **Emit receipt**
    - `ReceiptWriter.emit({ action: "post_review_comment", pr_url, review_text, status: "success" })`

## Expected Artifacts
- `before-pr-page.png` — screenshot of PR landing page
- `diff-view.png` — screenshot of Files Changed tab
- `review-form-filled.png` — screenshot of filled review form before submission
- `review-submitted.png` — screenshot confirming posted review
- `receipt.json` — action receipt with integrity hash, pr_url, review_text excerpt, and status

## Failure Handling
- **Navigation fails / 404**: emit receipt `status: error`, `reason: pr_not_found`. Abort.
- **Auth expired (redirect to login)**: emit receipt `status: error`, `reason: auth_required`. Do not attempt login. Abort.
- **Review form not found**: fall back to the planning loop (vision model driving `browser.playwright`) for adaptive selector resolution. If still failing after 2 retries, abort with `status: error`, `reason: ui_changed`.
- **Approval denied**: emit receipt `status: blocked`, `reason: operator_denied`. No comment is posted.
- **Submission timeout (>30 s)**: screenshot current page as `timeout-state.png`, abort with `status: error`, `reason: timeout`.

## Conformance
Suite: `plugin-github-v1`
- PG-01: navigate to PR page and extract metadata
- PG-02: open Files Changed tab and extract diff
- PG-03: compose review via prompt
- PG-04: post review comment and verify submission
- PG-05: receipt emitted with integrity hash
- PG-06: policy approval gate fires for destructive action
