# Task PRLINK — linked PRs per code session (gizzi-code)
Read docs/CODE_PARITY_MAP.md first. Your tag: PRLINK. Build Contract A exactly in `cmd/gizzi-code`.

1. Find where gizzi stores session data and where tool results are finalized (bash tool, MCP tool results). Add a PR detector: parse GitHub PR URLs (`https://github.com/<o>/<r>/pull/<n>`) from tool outputs where the command created/pushed/viewed a PR in this session (`gh pr create`, `gh pr view`, `git push` output containing "/pull/" or "pull/new" — for `pull/new/<branch>` hints do NOT record, only real PR numbers), plus GitHub MCP create_pull_request results. Persist on the session (dedup by url). Don't record PR URLs that merely appear in file reads or web fetches.
2. Status fetcher: `gh api repos/<o>/<r>/pulls/<n>` when gh auth works, else REST with an existing stored GitHub token if gizzi has one; map to LinkedPR (state merged when merged_at, draft, mergeable conflict when mergeable===false or mergeable_state==='dirty'). 60s cache, background refresh, never blocks a turn, errors keep last values.
3. Routes exactly as Contract A in the server route files next to existing `/v1/session` routes + OpenAPI/SDK types if gizzi generates a client (update the generated types file by hand if generation needs a build — note it in NOTES).
4. Bus event `session.prs.updated`.
5. Docs: `surfaces/docs/` page for linked PRs in Code (how PRs get linked, badges, hover list, Open PR (G), refresh, manual link) + nav. Also document the session row ⋯ menu and its shortcuts (Open PR G, Pin P, Mark as unread U, Rename R, Copy link C, Archive A, Delete D, plus Mark as completed, Share, Move to, Move to project, Continue as project, Open in) and repo grouping — in the Code docs section.

## What this can break
cmd/gizzi-code/src → Desktop, Factory, gizzi CLI/TUI, Memory Drive, Voice; journeys "Send a message, get a streamed reply" (5,6), "Text your bot" (5). The detector must be cheap and must never throw into the tool pipeline (wrap, log, continue).
