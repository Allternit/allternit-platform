# Invisible Dots native AAI adapter

Primary source: https://github.com/feder-cr/invisible_dots/tree/a2584f26195062628263f465b4b3ec62939c60d0

Uses the authenticated control plane at http://127.0.0.1:8787. Override SUBS_GATEWAY_INVISIBLE_DOTS_URL and supply the user's server token through the sealed account credential; deployment fallback SUBS_GATEWAY_INVISIBLE_DOTS_TOKEN is optional. Never read or print a user's token from disk.

GET /api/dots discovers actual Dots. POST /api/dots/:id/messages submits a conversation message; GET conversation history matches assistant in_reply_to to the returned message_id. Queued delivery is not treated as a finished reply. Native mutation idempotency is unavailable: Allternit retains send results and uncertain outcomes in-process and does not retry automatically. Same send ID with changed text is refused.

Shared conversation; one Allternit send per account/Dot at a time. Closing detaches without stopping the vendor VM or shared job. Task approvals cannot be attributed to these conversation messages, so approval forwarding and cancellation remain unsupported. Source-based transport tests pass; live account verification is pending.
