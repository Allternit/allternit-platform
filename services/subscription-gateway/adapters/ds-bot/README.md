# DS Bot adapter

This local adapter connects to the DeepSeek Bot plugin hosted by DSH. Configure `SUBS_GATEWAY_DS_BOT_URL` with the explicit loopback origin for that DSH instance and `SUBS_GATEWAY_DS_BOT_TOKEN` with its 43-character Web launch token. The URL has no guessed port or fallback; it must be a loopback origin. The DSH helper exchanges the launch token for its signed session cookie.

Discovery reads `/api/bot` state and binds each visible bot to its existing `sessionId`. Sending calls DSH's native `session/prompt` RPC with the caller's correlation ID as `requestId`, then polls exact session projections and history pages until it finds the matching persisted user message and a completed turn. This preserves caller attribution and ensures an accepted receipt is not mistaken for a completed reply. The plugin's own send route is not used.

The adapter reports shared, best-effort context ownership and polling completion events. It does not claim approvals, cancellation, streaming, tools, computer control, or cloud availability. Uncertain prompt outcomes are returned as non-retryable; inspect the bot conversation before retrying.
