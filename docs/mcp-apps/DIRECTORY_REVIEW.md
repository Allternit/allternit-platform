# MCP App directory: review policy and pipeline

Backend for listing MCP Apps: submissions, review, held updates and installs. Code: `mcp_directory_routes.rs`,
`mcp_directory_held.rs`, `mcp_directory_guard.rs`, `migrations/V198__mcp_app_directory.sql`. **Migration V198 is not applied
in production**; see [OPERATIONS.md](OPERATIONS.md).

## Pipeline

`draft` → `in_review` → `approved` → `published`, or `in_review` → `rejected` → resubmit. Illegal transitions return 409.

1. **Domain check.** `POST /v1/developers/domain-tokens {host}` issues a token (shown once, only its SHA-256 stored, per developer
   and host; re-issuing replaces it and un-verifies). The developer serves it at `https://<host>/.well-known/allternit-apps-challenge`,
   then `POST .../check` fetches it server-side and compares the hash.
2. **Submit.** `POST /v1/miniapps/submissions` validates the form: name ≤ 30 chars, short description ≤ 80, long ≤ 4000, developer,
   https privacy and terms URLs, an icon, at least one screenshot, **5 positive and 3 negative test cases**. Failure is a 422 with
   findings and nothing is stored. Body limit 72 MiB. The state starts as `in_review`. Reviewer credentials go to a separate sealed table.
3. **Review.** An admin calls `POST /v1/miniapps/submissions/:id/review` with `approve` or `reject`; reject needs a reason.
   Approve stores the approved tool snapshot. Reviewers read credentials through `GET .../reviewer-credentials`, the only read path.
4. **Publish.** `POST /v1/miniapps/:id/publish` works only from `approved` (409 otherwise). The owner or an admin may call it.
5. **Held updates.** Once approved or published, a resubmission does not change the live listing. New or changed tools (title,
   description, `inputSchema`, annotations, `_meta`) and changed `ui://` resource metadata stay pending, and the live form,
   package and snapshot are untouched, until an admin calls `POST /v1/miniapps/:id/held/approve`. Removals only narrow scope and take effect at once.

The submission should carry a `snapshot` (the scanner's tool listing). Without it the record gets a `snapshot.missing` warning
and tool changes cannot be diffed for that version.

## Reviewer-org gate

Review, reviewer credentials, held-approve and `GET /v1/miniapps/submissions?scope=all` need **both**:

- `organization_id` equal to `ALLTERNIT_DIRECTORY_REVIEW_ORG_ID`, and
- an admin role in that org (`admin`/`owner`, with or without the `org:` prefix).

Any Clerk user can create an org and be its admin, so a role alone is not enough. **If the variable is unset, nobody can review**
(fail closed). Non-reviewers get 403 on these routes and 404 on other people's submissions.

## Installs and connector OAuth

`GET|PUT /v1/mcp-app-installs`, `PATCH|DELETE /v1/mcp-app-installs/:appId` are scoped to `user_id`, and the connector must be the
caller's. Permission mode is one of `always_ask`, `ask_before_changes` (default), `ask_before_important_changes`.
`POST /mcp/connectors/:id/oauth/start` returns an authorize URL with PKCE S256 and `resource`. `GET /oauth/client.json` (public)
is our client ID metadata document.

## SSRF guard

Domain checks and OAuth discovery fetches go through `mcp_directory_guard.rs`: https only, default port only, no IP literals,
name screening (`localhost`, `.local`, `.internal`, …), every DNS answer must be public, the connection is pinned to the
validated address, redirects are never followed, 5 s timeout, 4 KiB body cap. This is stricter than the connector guard, and it
has no dev bypass, so local connectors cannot use `oauth/start`.

## Not done

- The server does not unzip `packageZip`, scan it, or call the submitted MCP server. Review is manual.
- No route to reject a held update, and no unpublish route (the state machine supports unpublish).
- Directory state is separate from `services/registry/apps-registry`; they are not synced.
- Existing localStorage installs are not migrated; the client must move to these endpoints.
- No live test of the router nesting (`/v1` and `/api/v1`) against a running server.
