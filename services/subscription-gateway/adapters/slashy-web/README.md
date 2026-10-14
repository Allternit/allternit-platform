# slashy-web

Slashy through its web app (`ui_bridge_web`, browser sign-in). Provider id
`slashy`, pool `slashy-web`, origin `https://www.slashy.com`.

Route: 2 (browser sign-in). Slashy publishes no developer API. Its web app at
www.slashy.com uses Firebase sign-in; every non-marketing route is the signed-in
app (checked 2026-10-14). The user signs in once in Allternit's login browser.

**Selectors and the session cookie are v1 guesses** until the first real login is
inspected; `readAccount` reports no identity yet.

**Live check (owner-driven):** confirm the sign-in page,
update `manifest.yaml` (origins, login_url, session_cookies) and
`selectors/v1.yaml`, then add **Slashy** in Settings → Subscriptions and sign in
by hand in the login browser.
