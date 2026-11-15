/**
 * Review presets: `/review security|perf|tests|diff [target]` in the TUI and
 * `gizzi run --command review "security [target]"` in the CLI. Each preset is
 * a focused prompt; all of them report in the same findings format so results
 * read the same whichever one ran (the Claude Code / Codex review shape).
 *
 * Target ($ARGUMENTS): empty = uncommitted changes plus this branch's commits
 * since it left the default branch; a branch, commit, or PR number narrows it.
 */
export type ReviewPresetName = "security" | "perf" | "tests" | "diff"

const SCOPE = `## What to review

Target: "$ARGUMENTS"

- Empty target: review uncommitted changes (\`git diff\`, \`git diff --cached\`, \`git status --short\` for new files) and this branch's commits (\`git diff $(git merge-base HEAD origin/HEAD 2>/dev/null || git merge-base HEAD origin/main)...HEAD\`).
- A commit: \`git show <commit>\`. A branch: \`git diff <branch>...HEAD\`. A PR number or URL: \`gh pr diff <pr>\`.
- Read the surrounding code of each changed file before you flag anything. Review only; do not edit files.`

const FINDINGS = `## How to report

Start with one line: what was reviewed (files, commits) and the preset.

Then a "Findings" list, most severe first. Each finding is exactly:

\`[high|medium|low] path/to/file.ts:LINE — short title\`
  Why: one sentence on the concrete failure (input, state, or caller that breaks).
  Fix: one sentence on the change.

Only report findings you can tie to a line in the change. No style nits, no speculative "consider" items, no praise. If there are none, write "No findings."

End with one line: \`Verdict: PASS\` or \`Verdict: NEEDS WORK (N high, N medium, N low)\`.`

const FOCUS: Record<ReviewPresetName, { title: string; description: string; focus: string }> = {
  security: {
    title: "Security review",
    description: "Security review of the changes (injection, auth, secrets, unsafe input)",
    focus: `Look only for security problems the change introduces or exposes:
- Injection: shell, SQL, path traversal, template/HTML (XSS), unsafe deserialization, SSRF.
- Auth and access: missing or wrong authorization checks, IDOR, privilege escalation, CSRF.
- Secrets: keys/tokens in code, logs, errors or client bundles; weak randomness for tokens.
- Untrusted input reaching file system, process spawn, network or eval without validation.
- Crypto misuse and unsafe defaults (TLS off, permissive CORS, debug on).
Skip denial-of-service from huge inputs and theoretical issues with no reachable path.`,
  },
  perf: {
    title: "Performance review",
    description: "Performance review of the changes (hot paths, N+1, blocking, memory)",
    focus: `Look only for performance problems the change introduces:
- Work in hot paths or render loops that grows with input: nested loops, repeated parsing, N+1 queries or requests.
- Blocking or serial I/O that could run concurrently; sync file/network calls on the main thread or event loop.
- Unbounded caches, listeners or buffers; leaks from missing cleanup (timers, subscriptions, processes).
- Needless re-renders, recomputation without memoization where it's measurably hot, large copies.
- Missing indexes or full scans for new queries; pagination dropped.
Say how it scales (e.g. "O(n²) in number of messages") instead of guessing timings.`,
  },
  tests: {
    title: "Test review",
    description: "Test review of the changes (untested paths, weak or wrong tests)",
    focus: `Look only at whether the change is tested well:
- Changed logic with no test that would fail if it broke (name the case: input, branch, error path).
- Tests that cannot fail: no assertion, asserting on mocks of our own code, snapshots of markup, skipped tests.
- Tests that assert the wrong thing or pass for the wrong reason; flaky timing or order dependence.
- Bug fixes without a regression test that reproduces the bug.
For each missing test, say the exact case to add and where. Do not ask for tests of plain wiring or glue code.`,
  },
  diff: {
    title: "Diff review",
    description: "Correctness review of the diff (bugs, broken callers, edge cases)",
    focus: `Look only for correctness bugs in the diff:
- Logic errors, wrong conditions, off-by-one, null/undefined and empty cases, wrong units.
- Callers and siblings the change breaks: changed signatures, renamed fields, call sites not updated.
- Error handling: swallowed errors, partial writes, missing cleanup, races between async steps.
- Behaviour changes the description does not mention.`,
  },
}

export const REVIEW_PRESETS = Object.keys(FOCUS) as ReviewPresetName[]

export function reviewPresetPrompt(name: ReviewPresetName): string {
  const p = FOCUS[name]
  return `You are doing a focused ${p.title.toLowerCase()}.\n\n${p.focus}\n\n${SCOPE}\n\n${FINDINGS}\n`
}

export function reviewPresetDescription(name: ReviewPresetName): string {
  return FOCUS[name].description
}

/** "security main" → { name: "security", target: "main", prompt } ; anything else → undefined. */
export function matchReviewPreset(args: string | undefined): { name: ReviewPresetName; target: string; prompt: string } | undefined {
  const m = /^\s*(security|perf|performance|tests?|diff)\b\s*([\s\S]*)$/i.exec(args ?? "")
  if (!m) return undefined
  const word = m[1]!.toLowerCase()
  const name: ReviewPresetName = word.startsWith("perf") ? "perf" : word.startsWith("test") ? "tests" : (word as ReviewPresetName)
  const target = m[2]!.trim()
  return { name, target, prompt: reviewPresetPrompt(name).replaceAll("$ARGUMENTS", target) }
}
