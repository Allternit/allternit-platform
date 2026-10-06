import type { ContentBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/messages.js'
import type { Command } from '../../commands.js'
import { ALLTERNIT_GATEWAY_BASE } from '@/shared/constants/allternitGateway'

const API_BASE = process.env.Allternit_API_URL || ALLTERNIT_GATEWAY_BASE
const FACTORY_BASE = `${API_BASE}/api/factory`

type ReceiptKind =
  | 'tool_call_post'
  | 'validator_report'
  | 'build_report'
  | 'gate_decision'
  | 'session_start'
  | 'dag_load'
  | 'node_entry'
  | 'context_pack_sealed'
  | string

interface Receipt {
  receiptId: string
  kind: ReceiptKind
  runId?: string
  dagId?: string
  nodeId?: string
  wihId?: string
  timestamp?: string
  payload?: unknown
  signature?: string
}

interface QueryOptions {
  dagId?: string
  nodeId?: string
  wihId?: string
  kinds?: ReceiptKind[]
  limit?: number
}

function parseArgs(args: string): { subcommand: string; id?: string; options: QueryOptions } {
  const tokens = args.trim().split(/\s+/).filter(Boolean)
  const result: { subcommand: string; id?: string; options: QueryOptions } = {
    subcommand: 'list',
    options: { limit: 20 },
  }

  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i]
    if (token === 'show' && i === 0) {
      result.subcommand = 'show'
      result.id = tokens[i + 1]
      break
    }
    if (token === '--dag' || token === '-d') {
      result.options.dagId = tokens[++i]
    } else if (token === '--node' || token === '-n') {
      result.options.nodeId = tokens[++i]
    } else if (token === '--wih' || token === '-w') {
      result.options.wihId = tokens[++i]
    } else if (token === '--kind' || token === '-k') {
      const kind = tokens[++i]
      if (kind) {
        result.options.kinds = result.options.kinds || []
        result.options.kinds.push(kind)
      }
    } else if (token === '--limit' || token === '-l') {
      result.options.limit = Number.parseInt(tokens[++i], 10) || 20
    }
  }

  return result
}

async function queryReceipts(options: QueryOptions): Promise<string> {
  const body: Record<string, unknown> = { limit: options.limit ?? 20 }
  if (options.dagId) body.dag_id = options.dagId
  if (options.nodeId) body.node_id = options.nodeId
  if (options.wihId) body.wih_id = options.wihId
  if (options.kinds && options.kinds.length > 0) body.kinds = options.kinds

  const res = await fetch(`${FACTORY_BASE}/receipts`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })

  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new Error(`Receipts query failed (${res.status} ${res.statusText}): ${text}`)
  }

  const data = await res.json().catch(() => ({}))
  const receipts: Receipt[] = data.receipts || []

  if (receipts.length === 0) {
    return 'No receipts matched the query.'
  }

  const lines = receipts.map((r) => {
    const ts = r.timestamp ? new Date(r.timestamp).toLocaleString() : '—'
    return `- **${r.receiptId.slice(0, 20)}…** | ${r.kind} | dag=${r.dagId || '—'} | node=${r.nodeId || '—'} | wih=${r.wihId || '—'} | ${ts}`
  })

  return `**${receipts.length} receipt(s)**\n\n${lines.join('\n')}`
}

async function showReceipt(id: string): Promise<string> {
  const res = await fetch(`${FACTORY_BASE}/receipts/${encodeURIComponent(id)}`)
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new Error(`Receipt fetch failed (${res.status} ${res.statusText}): ${text}`)
  }
  const r = (await res.json()) as Receipt
  const payload = typeof r.payload === 'string' ? r.payload : JSON.stringify(r.payload, null, 2)
  return `**Receipt ${r.receiptId}**\n- kind: ${r.kind}\n- dag: ${r.dagId || '—'}\n- node: ${r.nodeId || '—'}\n- wih: ${r.wihId || '—'}\n- timestamp: ${r.timestamp || '—'}\n- signature: ${r.signature ? `${r.signature.slice(0, 32)}…` : '—'}\n\n\`\`\`json\n${payload}\n\`\`\``
}

const receipts: Command = {
  type: 'prompt',
  name: 'receipts',
  aliases: ['receipt'],
  description: 'Query and inspect DAG execution receipts',
  progressMessage: 'querying receipts',
  contentLength: 0,
  source: 'builtin',
  async getPromptForCommand(args): Promise<ContentBlockParam[]> {
    const { subcommand, id, options } = parseArgs(args)

    try {
      if (subcommand === 'show') {
        if (!id) {
          return [{ type: 'text', text: 'Usage: /receipts show <receipt-id>' }]
        }
        return [{ type: 'text', text: await showReceipt(id) }]
      }
      return [{ type: 'text', text: await queryReceipts(options) }]
    } catch (err) {
      return [{ type: 'text', text: `Receipts command failed: ${err instanceof Error ? err.message : String(err)}` }]
    }
  },
}

export default receipts
