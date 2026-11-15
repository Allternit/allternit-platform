---
description: Read the newest one-time code from an Allternit mailbox or number
argument-hint: "[mailbox address or number, optional]"
---

Use the allternit-phones skill and the `allternit` MCP server.

Target: $ARGUMENTS

1. If the target is an email address, or empty, use `mailbox_list` to pick the mailbox (ask me if
   there are several), then `mail_read_otp`. Retry every 5 seconds for up to a minute if nothing
   has arrived, then show `mail_list` for that mailbox.
2. If the target is a phone number held by the project, read the newest inbound text for it from
   `GET /v1/messages?number_id=...&direction=inbound`.
3. Only read texts on my own phone with `phone.sms.read` if I ask; it needs my approval on Desktop.
4. Show the code and who sent it. Don't store it anywhere.
