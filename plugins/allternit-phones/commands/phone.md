---
description: Do something on one of your Allternit phones (screenshot, tap, open an app, or run a goal)
argument-hint: "[goal, e.g. open Settings and turn on Wi-Fi]"
---

Use the allternit-phones skill and the `allternit` MCP server.

1. Call `phones_list`. If no phone is online, or the Desktop is offline, stop and tell me what to fix.
2. If more than one phone is online, ask me which serial to use.
3. Goal: $ARGUMENTS
   - No goal given: take a `phone_screenshot` and describe the screen.
   - Short goal (a few steps): drive it with screenshot, then one action, then screenshot.
   - Longer goal: start `phone_task` with the goal, give me the `phone_live_url` link, and poll
     `phone_run_status` until it finishes.
4. Report what happened in plain words. Ask before anything that sends, buys, posts, deletes or
   changes settings, and remind me to approve any text or call on my Desktop.
