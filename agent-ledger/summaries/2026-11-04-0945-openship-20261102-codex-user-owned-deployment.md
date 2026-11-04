# OpenShip integration release attestation

User authorization: Eoj approved release checks, commits, pushes, and merges with “go”. Hosting is supplied and paid for by users. No hosting was purchased or customer project deployed.

App PR https://github.com/Allternit/allternit-ai/pull/969 merged as 894cf89daa596043edc49bbf04ee5bb62a0f5a62. Docs PR https://github.com/Allternit/allternit-platform/pull/2090 merged as 6b55743d35b372dd3811ac8c703f7559eacf618e.

Settings Infrastructure and Code projects expose the same authenticated MCP-based deployment panel on Desktop/web/phone/PWA. Includes discovery, explicit reviews for writes, rollback plan inspection, status/log reads, tool capability denial, and ambiguous-write recovery without automatic retries. Runtime proxy routing covers saved MCP connectors.

Validation: 42 tests across six suites passed in one run; targeted ESLint and diff checks passed. Exact app commit CI typecheck and production build passed (run 41858785941). Docs CI passed (41858762004), local link check returned zero problems, dependency map validated. Fixture screenshots covered desktop and 390px phone with no horizontal overflow. Screenshots are fixtures, not provider acceptance.

Limits: no user-owned OpenShip installation was supplied, so live provider deployment is unverified. The feature map remains building. Required steering consults returned CONSULT_FAILED under the documented fail-open policy, not approval. Local fallback tsc was stopped after prolonged machine contention; CI typecheck is the completed check. Both shared checkouts were fast-forwarded after merge using the documented orchestrator environment escape.

At attestation: PWA and docs production deployment jobs are running; ai web fallback workflow dispatched. Desktop build queue need returned exit 3: no finished or pending Desktop build contains the app merge. Desktop follows the normal release train; no competing heavy build was started. Production deployment status will be recorded in the session artifact.

Agent: Codex. No provider tokens are stored in the panel. No Allternit hosting subsidy.
