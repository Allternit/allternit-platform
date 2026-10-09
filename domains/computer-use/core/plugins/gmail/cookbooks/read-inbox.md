# Cookbook: Read Gmail Inbox and Summarise Unread Messages

## Goal
Open Gmail, identify the first 5 unread messages in the inbox, extract their sender,
subject, and body preview, then produce a structured summary of each message.

## Preconditions
- The agent has an active Google session in the browser profile (already signed in to Gmail).
- Gmail is accessible at `https://mail.google.com`.
- At least 1 unread message exists in the inbox.
- No 2-factor authentication challenge is pending.

## Family
browser

## Mode
inspect

## Primary Adapter
browser.playwright

## Fallback
planning loop (vision model driving browser.playwright)

## Policy
- Plugin policy profile: `gmail` (max_destructive_actions: 2, requires_approval: true)
- This cookbook operates in **inspect** mode — it reads but does not send or delete.
- No destructive actions are taken; operator approval is not required for this cookbook.
- Navigation is restricted to `mail.google.com` and `accounts.google.com`.
- If a login redirect is detected, the cookbook aborts (it does not handle credentials).

## Steps

1. **Navigate to Gmail inbox**
   - `goto(url="https://mail.google.com/mail/u/0/#inbox")`
   - Wait for `div[role='main']` to be visible (up to 15 s).
   - If redirected to `accounts.google.com`, abort: `status: error`, `reason: auth_required`.
   - Screenshot: `inbox-landing.png`

2. **Dismiss overlays** (if present)
   - If `div[data-dialog-type]` or `div[role='dialog']` is visible:
     - `click(selector="button[data-action='cancel'], button[aria-label='Close']", optional=true)`
   - Wait 500 ms for overlay to dismiss.

3. **Find unread message rows**
   - `extract(selector="tr.zA.zE", multiple=true, limit=10)` → list of unread row elements
     (Gmail marks unread with class `zE` in addition to `zA` for all rows)
   - If fewer than 1 unread row found: emit receipt `status: ok`, `reason: no_unread_messages`. Stop.
   - Take up to 5 rows: `unread_rows = unread_rows[:5]`

4. **For each unread row (index 0..N-1)**

   a. **Extract list-view metadata from the row** (without opening the message)
      - Sender: `extract(selector="tr.zA.zE:nth-child({i+1}) span.zF")` → sender name/email
      - Subject: `extract(selector="tr.zA.zE:nth-child({i+1}) span.bog")` → subject line
      - Snippet: `extract(selector="tr.zA.zE:nth-child({i+1}) span.y2")` → body preview snippet
      - Time: `extract(selector="tr.zA.zE:nth-child({i+1}) span.xW span")` → relative time
      - Store: `messages[i] = { sender, subject, snippet, time, full_body: null }`

   b. **Open the message**
      - `click(selector="tr.zA.zE:nth-child({i+1})")`
      - Wait for `div.a3s.aiL` (message body container) to be visible (up to 10 s).
      - Screenshot: `message-{i}-open.png`

   c. **Extract full message body**
      - `extract(selector="div.a3s.aiL", inner_text=true)` → `full_body` (truncated to 1500 chars)
      - Update: `messages[i].full_body = full_body`

   d. **Go back to inbox**
      - `click(selector="div[aria-label='Back to Inbox'], a[href='#inbox']")`
      - Wait for `div[role='main']` inbox view to be visible again.

5. **Compose summaries** (LLM step — one call per message)
   - For each `messages[i]`:
     - Prompt: `prompts/summarize-email.txt` with variables:
       `{{ sender }}`, `{{ subject }}`, `{{ snippet }}`, `{{ full_body }}`
     - Output: `messages[i].summary` (1-3 sentences, plain text)

6. **Compile inbox report**
   - Structure:
     ```json
     {
       "unread_count_visible": N,
       "retrieved_at": "<ISO-8601 timestamp>",
       "messages": [
         {
           "index": 0,
           "sender": "...",
           "subject": "...",
           "time": "...",
           "summary": "..."
         },
         ...
       ]
     }
     ```
   - Write report to `inbox-summary.json` artifact.

7. **Final screenshot**
   - Navigate back to inbox if not already there.
   - Screenshot: `inbox-final.png`

8. **Emit receipt**
   - `ReceiptWriter.emit({ action: "read_inbox", messages_read: N, status: "success" })`

## Expected Artifacts
- `inbox-landing.png` — screenshot of inbox on first load
- `message-0-open.png` through `message-{N-1}-open.png` — one screenshot per opened message
- `inbox-final.png` — screenshot of inbox after completing reads
- `inbox-summary.json` — structured report of the N messages with summaries
- `receipt.json` — action receipt with integrity hash and message count

## Failure Handling
- **Auth redirect**: abort immediately with `status: error`, `reason: auth_required`. Do not attempt login.
- **Gmail UI structure changed** (selectors return empty): fall back to the planning loop (vision model driving `browser.playwright`) for visual extraction. If still failing, emit `status: partial`, populate what was extractable.
- **Message body not visible after 10 s**: skip that message, record `status: timeout` in its entry, continue with next.
- **LLM summarisation fails**: store raw `snippet` as the summary field, mark `summary_source: snippet_fallback`.
- **All 5 messages fail to open**: emit receipt `status: error`, `reason: all_messages_failed`.

## Conformance
Suite: `plugin-gmail-v1`
- PM-01: navigate to inbox without triggering auth
- PM-02: identify and count unread rows
- PM-03: open each message and extract body
- PM-04: produce summary per message via prompt
- PM-05: compile and write inbox-summary.json
- PM-06: receipt emitted with correct message count
