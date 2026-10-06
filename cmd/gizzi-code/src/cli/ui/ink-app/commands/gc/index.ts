import type { ContentBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/messages.js'
import type { Command } from '../../commands.js'
import { ALLTERNIT_GATEWAY_BASE } from '@/shared/constants/allternitGateway'

const API_BASE = process.env.Allternit_API_URL || ALLTERNIT_GATEWAY_BASE
const FACTORY_BASE = `${API_BASE}/api/factory`

interface WihInfo {
  wih_id: string
  node_id?: string
  dag_id?: string
  status?: string
  title?: string
}

interface GateVerifyResult {
  ok: boolean
  ledger_chain_ok: boolean
  ledger_chain_issues?: string[]
  cycle_dags: string[]
}

async function loadStatus(): Promise<string> {
  const [wihsRes, vaultRes, gateRes, rulesRes] = await Promise.all([
    fetch(`${FACTORY_BASE}/wihs`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({}) }),
    fetch(`${FACTORY_BASE}/vault/status`),
    fetch(`${FACTORY_BASE}/gate/verify`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({}) }),
    fetch(`${FACTORY_BASE}/gate/rules`),
  ])

  const wihsData = await wihsRes.json().catch(() => ({}))
  const vaultData = await vaultRes.json().catch(() => ({}))
  const gateData = await gateRes.json().catch(() => ({}))
  const rulesData = await rulesRes.json().catch(() => ({}))

  const wihs: WihInfo[] = wihsData.wihs || []
  const closed = wihs.filter(w => w.status === 'closed')
  const jobs = vaultData.jobs || []
  const verify: GateVerifyResult = gateData

  let text = `**GC Agents status**\n`
  text += `- Closed WIHs: ${closed.length} / ${wihs.length}\n`
  text += `- Vault jobs: ${jobs.length}\n`
  text += `- Gate verification: ${verify.ok ? 'passed' : 'failed'} (ledger_chain_ok=${verify.ledger_chain_ok ?? false})\n`
  text += `- Cycle DAGs: ${(verify.cycle_dags || []).length}\n`
  if (verify.ledger_chain_issues && verify.ledger_chain_issues.length > 0) {
    text += `- Ledger issues:\n${verify.ledger_chain_issues.map(i => `  - ${i}`).join('\n')}\n`
  }
  if (rulesData.rules) {
    text += `\nGate rules:\n\`\`\`\n${rulesData.rules}\n\`\`\``
  }
  return text
}

async function archiveWih(wihId: string): Promise<string> {
  const res = await fetch(`${FACTORY_BASE}/vault/archive`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ wih_id: wihId }),
  })
  if (!res.ok) {
    const body = await res.text().catch(() => '')
    throw new Error(`Archive failed (${res.status} ${res.statusText}): ${body}`)
  }
  return `WIH ${wihId} archived.`
}

async function verifyGraph(): Promise<string> {
  const res = await fetch(`${FACTORY_BASE}/gate/verify`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({}),
  })
  if (!res.ok) {
    throw new Error(`Gate verify failed (${res.status} ${res.statusText})`)
  }
  const data = (await res.json()) as GateVerifyResult
  return `Gate verification: ${data.ok ? 'passed' : 'failed'}. ledger_chain_ok=${data.ledger_chain_ok}, cycle_dags=${data.cycle_dags.length}`
}

async function rebuildIndex(): Promise<string> {
  const res = await fetch(`${FACTORY_BASE}/index/rebuild`, { method: 'POST' })
  if (!res.ok) {
    throw new Error(`Index rebuild failed (${res.status} ${res.statusText})`)
  }
  const data = await res.json().catch(() => ({}))
  return `Index rebuilt. indexed_count=${data.indexed_count ?? 'unknown'}`
}

const gc: Command = {
  type: 'prompt',
  name: 'gc',
  aliases: ['gc-agents'],
  description: 'Run DAG garbage-collection, archive, and index-maintenance controls',
  progressMessage: 'running GC agent controls',
  contentLength: 0,
  source: 'builtin',
  async getPromptForCommand(args): Promise<ContentBlockParam[]> {
    const tokens = args.trim().split(/\s+/).filter(Boolean)
    const subcommand = tokens[0]?.toLowerCase() || 'status'

    try {
      switch (subcommand) {
        case 'status':
          return [{ type: 'text', text: await loadStatus() }]
        case 'archive': {
          const wihId = tokens[1]
          if (!wihId) {
            return [{ type: 'text', text: 'Usage: /gc archive <wih-id>' }]
          }
          return [{ type: 'text', text: await archiveWih(wihId) }]
        }
        case 'verify':
          return [{ type: 'text', text: await verifyGraph() }]
        case 'rebuild':
          return [{ type: 'text', text: await rebuildIndex() }]
        default:
          return [{ type: 'text', text: 'Usage: /gc [status|archive <wih-id>|verify|rebuild]' }]
      }
    } catch (err) {
      return [{ type: 'text', text: `GC command failed: ${err instanceof Error ? err.message : String(err)}` }]
    }
  },
}

export default gc
