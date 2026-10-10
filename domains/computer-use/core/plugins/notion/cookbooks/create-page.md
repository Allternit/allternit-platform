# Cookbook: Create a New Notion Page

## Goal
Open Notion, navigate to a target workspace or parent page, create a new page with a
given title, populate it with structured content (heading, body paragraphs, and a
to-do checklist), and screenshot the completed page as a receipt artifact.

## Preconditions
- The agent has an active Notion session in the browser profile (already signed in).
- The target workspace URL is known or the agent will create the page at the workspace root.
- The page title and desired content are provided as task inputs.
- Operator approval is configured for page creation (destructive action).

## Family
browser

## Mode
execute

## Primary Adapter
browser.playwright

## Fallback
planning loop (vision model driving browser.playwright)

## Policy
- Plugin policy profile: `notion` (max_destructive_actions: 2, requires_approval: true)
- Page creation is classified as a **destructive action** (creates persistent content).
- Operator approval is required before any content is typed.
- Navigation is restricted to `notion.so` and `*.notion.so`.
- `delete_workspace` and `delete_database` are blocked unconditionally.

## Steps

1. **Navigate to Notion workspace**
   - `goto(url="https://www.notion.so")`
   - Wait for the sidebar (`div[data-block-id]` or `nav[aria-label='Sidebar']`) to be visible (up to 20 s).
   - If redirected to `/login`, abort: `status: error`, `reason: auth_required`.
   - If a `target_parent_url` is provided:
     - `goto(url=target_parent_url)`
     - Wait for the page editor to load.
   - Screenshot: `notion-workspace.png`

2. **Request operator approval for page creation**
   - Emit `ApprovalRequest { action: "create_notion_page", title: page_title, parent: target_parent_url }`
   - Wait for `ApprovalResponse { approved: true }`.
   - If denied: emit receipt `status: blocked`, `reason: operator_denied`. Abort.

3. **Open the New Page command**
   - Strategy A (sidebar button):
     - `click(selector="div.notion-sidebar-container div[role='button'][aria-label='New page']", optional=true)`
   - Strategy B (keyboard shortcut):
     - If Strategy A fails, use `keyboard.press(keys="Meta+N")` (macOS) or `Ctrl+N` (Linux/Windows)
   - Wait for a blank page editor (`div.notion-page-content`) to appear (up to 10 s).
   - Screenshot: `new-page-blank.png`

4. **Type the page title**
   - `click(selector="div[placeholder='Untitled'], div[data-content-editable-leaf='true']")`
   - `type(selector="div[placeholder='Untitled']", text=page_title)`
   - Press `Enter` to move cursor into the body.
   - Screenshot: `title-typed.png`

5. **Add a H1 heading block**
   - Type `/heading1` → wait for slash-command menu → `click` "Heading 1" option.
     - Fallback: `type(text="# {{ page_heading }}")` if slash menu does not appear.
   - `type(text=page_heading)` after selecting the block type.
   - Press `Enter` to move to a new block.

6. **Add body paragraphs**
   - For each paragraph in `content_paragraphs` (list of strings):
     - Ensure cursor is on a blank line (press `Enter` if needed).
     - `type(text=paragraph)`
     - Press `Enter` to create a new block.

7. **Add a to-do checklist section**
   - Type `/todo` → wait for slash menu → `click` "To-do list" option.
   - For each item in `todo_items` (list of strings):
     - `type(text=item)`
     - Press `Enter` to create the next checkbox.
   - Screenshot: `content-added.png`

8. **Wait for auto-save**
   - Wait until the "Saving…" indicator disappears (up to 10 s).
   - Confirm by checking that no `div[aria-label='Saving']` is visible.
   - Screenshot: `page-saved.png`

9. **Copy the page URL**
   - `extract(selector="window.location.href")` → `new_page_url`

10. **Emit receipt**
    - `ReceiptWriter.emit({ action: "create_notion_page", page_title, new_page_url, status: "success" })`

## Expected Artifacts
- `notion-workspace.png` — screenshot of workspace on arrival
- `new-page-blank.png` — screenshot of the blank new page
- `title-typed.png` — screenshot after title is entered
- `content-added.png` — screenshot of the page with heading, body, and todos
- `page-saved.png` — screenshot confirming auto-save completed
- `receipt.json` — receipt with integrity hash, page_title, new_page_url, and status

## Failure Handling
- **Auth redirect on load**: abort with `status: error`, `reason: auth_required`.
- **Operator approval denied**: emit receipt `status: blocked`. Abort — no page is created.
- **New page command fails (both strategies)**: fall back to the planning loop (vision model driving `browser.playwright`) for visual "New page" button discovery. If still failing, abort with `status: error`, `reason: ui_changed`.
- **Slash command menu does not appear**: type block content as plain text without block formatting; note `formatting_degraded: true` in receipt.
- **Auto-save timeout (>10 s)**: screenshot current state as `save-timeout.png`. Emit receipt `status: partial`, `reason: save_unconfirmed`. Include `new_page_url` if extractable.
- **Page URL not extractable**: store `new_page_url: null` in receipt.

## Conformance
Suite: `plugin-notion-v1`
- PN-01: navigate to workspace without auth prompt
- PN-02: approval gate fires before page creation
- PN-03: new page created with correct title
- PN-04: heading, paragraphs, and todos all present in page body
- PN-05: auto-save confirmed before receipt emission
- PN-06: receipt contains new_page_url and integrity hash
