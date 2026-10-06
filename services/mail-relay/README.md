# allternit-mail-relay

Bot email on customer domains (`support@acme.com`). Runs on the mail host
`mx.allternit.com` next to Postfix:

| Port (loopback) | Who calls it | What |
|---|---|---|
| 2526 socketmap | Postfix | `relay_domain` / `recipient` lookups: only verified domains and existing bot mailboxes are accepted |
| 2525 SMTP | Postfix | accepted mail → the agent mail worker's `POST /api/v1/relay/inbound` (a worker error is a 451, so Postfix retries) |
| 8025 HTTP (nginx: `https://mx.allternit.com/relay/`) | the agent mail worker | `PUT/GET/DELETE /relay/domains/:host`, `POST /relay/domains/:host/check`, `POST /relay/send` (DKIM-signs, queues in Postfix), public `GET /relay/health` |

Every call between the worker and the relay carries `Authorization: Bearer <RELAY_SECRET>`
(the worker's `MAIL_RELAY_SECRET`). Domains and their DKIM keys live in
`DATA_DIR/domains.json` (mode 600). The full flow and DNS records are in the docs:
`surfaces/docs/api/allternit-bus.mdx` → Company domains.

```
npm test                       # unit tests
RELAY_SECRET=… WORKER_URL=… deploy/install.sh   # on the mail host, as root
```

The host needs reverse DNS `mx.allternit.com` for its IP and a DNS-only A record
`mx.allternit.com`. Postfix accepts nothing else (no local mailboxes, no open relay).
