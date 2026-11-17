# Custom Product Pages — Allternit for iPhone

Source for the four Custom Product Pages (plan step 13). Apple allows up to 70 per app. Promotional text limit is 170 characters. Numbers in brackets are character counts. Screenshots are real captures from the app, made at step 13 and inspected at full size before upload.

## Keyword rule

A Custom Product Page can only use keywords that are already in the app's keyword field in `listing.md` (see "Confirm the keyword picker rule" at step 13). The field today, entered for 1.0.1 on 2026-11-11 (99 of 100 characters):

`assistant,chatbot,desktop,automation,workflow,coding,tasks,approval,mac,pc,cloud,alternit,gizzi,llm`

Keywords in the field: assistant, chatbot, desktop, automation, workflow, coding, tasks, approval, mac, pc, cloud, alternit, gizzi, llm.

Not in the field: `remote` and `computer`. Both are in the subtitle ("Remote control your computer"), and Apple counts words from the name and subtitle as searchable, so page 1 can use them as long as the picker accepts subtitle words. Step 13 confirms what the picker offers. If it does not, page 1 uses desktop, mac and pc only.

## Question for Eoj: the "messages" keyword

The iMessage bots page needs the word `messages`, which is not in the field. To add it, swap out `workflow`:

`assistant,chatbot,desktop,automation,messages,coding,tasks,approval,mac,pc,cloud,alternit,gizzi,llm`

That field is 99 of 100 characters, the same as today (`workflow` and `messages` are both 8 letters). The plan said 96 of 100; the real count is 99 either way.

The cost: `workflow` leaves the field, so page 2 ("AI bots and agents") loses it from its set. Page 2 still has assistant, chatbot, automation, approval and llm. A keyword change only reaches search with a new app version, so this swap would ship with the next version, not instantly. Eoj decides. Until he does, page 3 uses only `chatbot` and `assistant`.

## Pages

| # | Page name | Promotional text (170) | Keywords | Screenshots (real captures) |
|---|---|---|---|---|
| 1 | Remote control your computer | Watch your computer's screen from your iPhone, take control, and approve what your bots ask. Works with Allternit Desktop on your Mac or PC. (140) | remote, computer, desktop, mac, pc | 1. Screen view 2. Take control 3. Approval |
| 2 | AI bots and agents | Your bots and agents in one list. Open a thread, check a task, and approve from the Inbox before anything that matters happens. (127) | assistant, chatbot, automation, workflow, approval, llm | 1. Bot list 2. Bot thread 3. Inbox |
| 3 | iMessage bots | Message your Allternit bots from the Messages app. They reply in the thread and ask before anything that matters. (113) | chatbot, assistant (add `messages` if Eoj approves the swap) | 1. Messages extension 2. A bot thread in Messages 3. An approval request |
| 4 | The open source alternative | Allternit, the open source alternative. Apache 2.0 code, the AI models you already use, and your own computer. (110) | alternit, gizzi, llm, assistant | 1. Tagline card 2. Models 3. Your computer |

Page 4 leads with "open source alternative" as the brand line, and the claim stays true: the code is Apache 2.0.

## Checks before upload

- Page 3: check the copy against the real Messages extension (what it shows, what a bot can do in a thread) before the page is saved.
- No page claims a result the app does not deliver. "Take control" and "approve" are features the app has; nothing promises speed, savings or rankings.
- Every screenshot is opened at full size and checked for overlap, clipping and wrapped text at 1024x768 view.
