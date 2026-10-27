# Docs writing

Use this when a change needs user docs or developer docs, or when you find a feature that has no docs. Docs ship in the same change as the feature. (This replaces the old `docs-writer` subagent.)

1. Read the code you are documenting and the existing docs around it. Document what the code actually does now, not what it was meant to do.
2. Find where the page belongs: the existing guide, its navigation entry, and the pages that link to it. Update the existing page before you add a new one.
3. Write plain, direct sentences. Match the existing pages' structure, headings and tone. No hype words and no promises of results.
4. Make every example runnable: real commands, real flags, real paths. Check that names match the code.
5. Update the navigation and any pages that should link to the new content.
6. Run the docs checks the repo has (for example the link checker and the MDX validator) and fix what they report.
7. In MDX: a bare `<word>` in prose and an unquoted `: ` in frontmatter break the build. Use backticks and quotes.
