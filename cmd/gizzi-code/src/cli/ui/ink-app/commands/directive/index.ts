import type { ContentBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/messages.js'
import type { Command } from '../../commands.js'
import { ALLTERNIT_GATEWAY_BASE } from '@/shared/constants/allternitGateway'

const API_BASE = process.env.Allternit_API_URL || ALLTERNIT_GATEWAY_BASE
const FACTORY_BASE = `${API_BASE}/api/factory`

interface PlanNewResponse {
  prompt_id?: string
  dag_id: string
  node_id?: string
}

interface PlanRenderResponse {
  dag_id: string
  format: string
  content: string
}

async function compileDirective(text: string): Promise<string> {
  const planRes = await fetch(`${FACTORY_BASE}/plan`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ text }),
  })

  if (!planRes.ok) {
    const body = await planRes.text().catch(() => '')
    throw new Error(`Plan creation failed (${planRes.status} ${planRes.statusText}): ${body}`)
  }

  const plan = (await planRes.json()) as PlanNewResponse
  const dagId = plan.dag_id

  const renderRes = await fetch(`${FACTORY_BASE}/plan/${encodeURIComponent(dagId)}/render?format=markdown`)
  if (!renderRes.ok) {
    return `Created DAG plan **${dagId}**. (Render unavailable: ${renderRes.status} ${renderRes.statusText})`
  }

  const rendered = (await renderRes.json()) as PlanRenderResponse
  return `Created DAG plan **${dagId}**:\n\n${rendered.content}`
}

const directive: Command = {
  type: 'prompt',
  name: 'directive',
  aliases: ['compile', 'dag-plan'],
  description: 'Compile a natural-language directive into a DAG plan',
  progressMessage: 'compiling directive into DAG plan',
  contentLength: 0,
  source: 'builtin',
  async getPromptForCommand(args): Promise<ContentBlockParam[]> {
    const text = args.trim()
    if (!text) {
      return [{ type: 'text', text: 'Usage: /directive <natural language directive> — compiles it into a DAG plan via the Allternit Factory.' }]
    }

    try {
      const result = await compileDirective(text)
      return [{ type: 'text', text: result }]
    } catch (err) {
      return [{ type: 'text', text: `Directive compilation failed: ${err instanceof Error ? err.message : String(err)}` }]
    }
  },
}

export default directive
