# claude-duet-web

Claude Duet (Anthropic's proactive agent, research preview) through claude.ai
on the web (`ui_bridge_web`, browser sign-in). Provider id `claude-duet`. Pool
`claude-duet-web`. Entry: `https://claude.ai/`, then **Duet**.

Duet is Anthropic's own bot product. It is not Claude Managed Agents, which is
the developer API for agents you build yourself (see `claude-managed-agents`).

Route: 2 (browser sign-in). Duet has no public API, so it is not route 1.
Reuses `ClaudeWebAdapter` (composer, replies, `readAccount`, banners) and
overrides only the entry point. Availability: Claude Pro and Max in the US, on
a waitlist; iOS and Android apps, and claude.ai on a computer.

What Duet does on its own side, and Allternit does not drive: the morning
brief, approvals ("Approve" / "Not now" before sending or paying), its cloud
computer and its connectors. Allternit sends messages into Duet's one thread
and reads the replies.

**Selectors are v1-unverified.** The Duet entry locator and the thread shape
are inferred from the public product page, not from a logged-in session. Duet
replies can arrive long after the send, so `chat.continue` is best-effort.

**Manual gate (human-driven):** Settings → Connections: add **Claude Duet**.
Sign in by hand in the login browser at `https://claude.ai/`. Open Duet once,
confirm it appears, then send one message and check the reply.
