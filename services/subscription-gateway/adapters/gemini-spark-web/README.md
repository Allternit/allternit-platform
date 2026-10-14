# gemini-spark-web

Gemini Spark (Google's 24/7 personal agent, beta) through the Gemini web app
(`ui_bridge_web`, browser sign-in). Provider id `gemini-spark`. Pool
`gemini-spark-web`. Entry: `https://gemini.google.com/app`, then the side
panel's **Spark** tab (not Chat).

Route: 2 (browser sign-in). No public Spark API was found, so it is not route 1.
Reuses `GeminiWebAdapter` (composer, replies, `readAccount`, banners) and
overrides only the fresh-task entry point. Spark availability: US Google AI
Ultra and Pro, not the EEA, UK, Switzerland or Nigeria (per Google's help page).

**Selectors are v1-unverified.** The Spark tab locator (`role=tab, name=Spark`)
and the Spark task composer are inferred from help and press pages, not from a
logged-in session. Spark runs tasks on cloud VMs, so `chat.continue` on a Spark
thread is best-effort until the live thread URL shape is confirmed.

**Manual gate (human-driven):** Settings → Subscriptions: add **Gemini Spark**.
Sign in by hand in the login browser at `https://gemini.google.com/app`. Open
the Spark tab once, confirm it appears, then send one task and check the reply.
Google advises not to put sign-in or payment details into Spark threads.
