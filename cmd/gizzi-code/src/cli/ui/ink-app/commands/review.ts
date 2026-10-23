// @ts-nocheck
import type { ContentBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/messages.js'
import type { Command } from '../commands.js'
import { isUltrareviewEnabled } from './review/ultrareviewEnabled.js'
import { getDirectConnectServerUrl, getSessionId } from '../bootstrap/state.js'
import { ReviewVerdict } from '../../../../runtime/review/verdict.js'

function reviewBaseUrl(): string {
  return getDirectConnectServerUrl() ?? 'http://localhost:4096'
}

// Local verdict review: the runtime's independent reviewer (fresh context,
// read-only tools) on what this session changed, else uncommitted changes.
async function localReview(body: Record<string, unknown>): Promise<{ status: number; data: any }> {
  const sessionID = getSessionId()
  const route = sessionID ? `/session/${encodeURIComponent(sessionID)}/review` : '/review'
  const response = await fetch(new URL(route, reviewBaseUrl()), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
    body: JSON.stringify(body),
  })
  if (!response.ok && response.status !== 202) throw new Error(`${response.status} ${response.statusText}`)
  return { status: response.status, data: await response.json() }
}

const VERDICT_PROMPT = (text: string) => `
An independent reviewer (fresh context, read-only) just reviewed the local changes. Its verdict:

${text}

Report this verdict to the user as-is, briefly. Do not start fixing anything unless the user asks; they can run /review pass to loop review → fix until PASS.
`

// Legal wants the explicit surface name plus a docs link visible before the
// user triggers, so the description carries "gizzi-code on the web" + URL.
const CCR_TERMS_URL = 'https://docs.gizziio.com/claude-code-on-the-web'

const LOCAL_REVIEW_PROMPT = (args: string) => `
      You are an expert code reviewer. Follow these steps:

      1. If no PR number is provided in the args, run \`gh pr list\` to show open PRs
      2. If a PR number is provided, run \`gh pr view <number>\` to get PR details
      3. Run \`gh pr diff <number>\` to get the diff
      4. Analyze the changes and provide a thorough code review that includes:
         - Overview of what the PR does
         - Analysis of code quality and style
         - Specific suggestions for improvements
         - Any potential issues or risks

      Keep your review concise but thorough. Focus on:
      - Code correctness
      - Following project conventions
      - Performance implications
      - Test coverage
      - Security considerations

      Format your review with clear sections and bullet points.

      PR number: ${args}
    `

const review: Command = {
  type: 'prompt',
  name: 'review',
  description: 'Review local changes (verdict), loop until PASS (/review pass [rounds]), or review a PR (/review <pr>)',
  argumentHint: '[pass [rounds] | local <what to check> | <pr>]',
  progressMessage: 'reviewing pull request',
  contentLength: 0,
  source: 'builtin',
  async getPromptForCommand(args): Promise<ContentBlockParam[]> {
    const trimmed = (args ?? '').trim()
    const pass = /^pass(?:\s+(\d+))?$/i.exec(trimmed)
    const local = /^local(?:\s+([\s\S]+))?$/i.exec(trimmed)
    if (pass) {
      if (!getSessionId()) return [{ type: 'text', text: 'Tell the user: /review pass needs an active session.' }]
      try {
        const { data } = await localReview({ untilPass: true, ...(pass[1] ? { maxRounds: Number(pass[1]) } : {}) })
        return [{
          type: 'text',
          text: `A review loop started (up to ${data.maxRounds ?? 5} rounds): an independent reviewer checks the changes, and its findings come back to you as "Fix these review findings:" turns until it passes. Reply with one short line acknowledging this. Do not edit files in this reply.`,
        }]
      } catch (error) {
        return [{ type: 'text', text: `Tell the user the review loop could not start (${error instanceof Error ? error.message : String(error)}).` }]
      }
    }
    if (!trimmed || local) {
      try {
        const { data } = await localReview(local?.[1] ? { instruction: local[1].trim() } : {})
        if (data.verdict) return [{ type: 'text', text: VERDICT_PROMPT(ReviewVerdict.format(data.verdict)) }]
        if (local) return [{ type: 'text', text: 'Tell the user: there are no local changes to review.' }]
      } catch (error) {
        if (local) {
          return [{ type: 'text', text: `Tell the user the local review failed (${error instanceof Error ? error.message : String(error)}).` }]
        }
        // No reviewer service: fall through to the PR flow as before.
      }
    }
    return [{ type: 'text', text: LOCAL_REVIEW_PROMPT(args) }]
  },
}

// /ultrareview is the ONLY entry point to the remote bughunter path —
// /review stays purely local. local-jsx type renders the overage permission
// dialog when free reviews are exhausted.
const ultrareview: Command = {
  type: 'local-jsx',
  name: 'ultrareview',
  description: `~10–20 min · Finds and verifies bugs in your branch. Runs in Gizzi Code on the web. See ${CCR_TERMS_URL}`,
  isEnabled: () => isUltrareviewEnabled(),
  load: () => import('./review/ultrareviewCommand.js'),
}

export default review
export { ultrareview }
