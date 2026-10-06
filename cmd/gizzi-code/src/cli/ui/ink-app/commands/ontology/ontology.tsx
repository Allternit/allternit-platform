// @ts-nocheck
import React, { useEffect, useMemo, useState } from 'react'
import { Box, Text } from '../../ink.js'
import { useKeybinding } from '../../keybindings/useKeybinding.js'
import type { LocalJSXCommandOnDone } from '../../types/command.js'
import { ALLTERNIT_GATEWAY_BASE } from '@/shared/constants/allternitGateway'

interface OntologyCommandProps {
  onDone: LocalJSXCommandOnDone
}

interface DagPlan {
  dag_id: string
  version?: string
  created_at?: string
  metadata?: {
    title?: string
    description?: string
  }
}

interface WihInfo {
  wih_id: string
  node_id?: string
  dag_id?: string
  status?: string
  title?: string
}

interface ContextPack {
  context_pack_id: string
  inputs?: {
    dag_id?: string
    node_id?: string
    wih_id?: string
  }
}

interface Receipt {
  receipt_id: string
  kind?: string
  wih_id?: string
}

interface OntologyData {
  dags: DagPlan[]
  wihs: WihInfo[]
  packs: ContextPack[]
  receipts: Receipt[]
  error: string | null
}

const API_BASE = process.env.Allternit_API_URL || ALLTERNIT_GATEWAY_BASE
const FACTORY_BASE = `${API_BASE}/api/factory`

async function loadOntology(): Promise<OntologyData> {
  try {
    const [plansRes, wihsRes, packsRes, receiptsRes] = await Promise.all([
      fetch(`${FACTORY_BASE}/plans`),
      fetch(`${FACTORY_BASE}/wihs`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({}) }),
      fetch(`${FACTORY_BASE}/context-packs`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ limit: 100 }) }),
      fetch(`${FACTORY_BASE}/receipts`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ limit: 200 }) }),
    ])

    const plansJson = await plansRes.json().catch(() => ({}))
    const wihsJson = await wihsRes.json().catch(() => ({}))
    const packsJson = await packsRes.json().catch(() => ({}))
    const receiptsJson = await receiptsRes.json().catch(() => ({}))

    return {
      dags: Array.isArray(plansJson) ? plansJson : plansJson.dags || [],
      wihs: wihsJson.wihs || [],
      packs: packsJson.context_packs || packsJson.packs || packsJson.contextPacks || [],
      receipts: receiptsJson.receipts || [],
      error: null,
    }
  } catch (err) {
    return {
      dags: [],
      wihs: [],
      packs: [],
      receipts: [],
      error: err instanceof Error ? err.message : 'Failed to load DAG ontology',
    }
  }
}

function useOntologyData() {
  const [data, setData] = useState<OntologyData | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    loadOntology().then((result) => {
      if (!cancelled) {
        setData(result)
        setLoading(false)
      }
    })
    return () => {
      cancelled = true
    }
  }, [])

  return { data, loading }
}

export function OntologyViewer({ onDone }: OntologyCommandProps): React.ReactNode {
  const { data, loading } = useOntologyData()

  useKeybinding(
    'confirm:no',
    () => {
      onDone()
    },
    { context: 'Confirmation' },
  )

  const stats = useMemo(() => {
    if (!data) return null
    const nodeIds = new Set<string>()
    for (const wih of data.wihs) {
      if (wih.node_id) nodeIds.add(wih.node_id)
    }
    for (const pack of data.packs) {
      if (pack.inputs?.node_id) nodeIds.add(pack.inputs.node_id)
    }
    return {
      dags: data.dags.length,
      nodes: nodeIds.size,
      wihs: data.wihs.length,
      packs: data.packs.length,
      receipts: data.receipts.length,
    }
  }, [data])

  return (
    <Box flexDirection="column" padding={1} borderStyle="single" borderColor="cyan">
      <Text color="cyan" bold>— Ontology Viewer — DAG runtime entities</Text>

      {loading || !data ? (
        <Box marginY={1}>
          <Text color="yellow">Loading ontology…</Text>
        </Box>
      ) : data.error ? (
        <Box marginY={1}>
          <Text color="red">Error: {data.error}</Text>
        </Box>
      ) : (
        <>
          <Box flexDirection="column" marginY={1}>
            <Text bold>Summary</Text>
            <Box gap={2}>
              <Text color="green">DAGs: {stats?.dags ?? 0}</Text>
              <Text color="blue">Nodes: {stats?.nodes ?? 0}</Text>
              <Text color="yellow">WIHs: {stats?.wihs ?? 0}</Text>
              <Text color="magenta">Packs: {stats?.packs ?? 0}</Text>
              <Text color="cyan">Receipts: {stats?.receipts ?? 0}</Text>
            </Box>
          </Box>

          <Box flexDirection="column" marginTop={1}>
            <Text bold>DAG Plans</Text>
            {data.dags.length === 0 ? (
              <Text color="gray">No DAG plans found.</Text>
            ) : (
              data.dags.slice(0, 12).map((dag, idx) => (
                <Box key={dag.dag_id || idx} gap={1}>
                  <Text color="gray">•</Text>
                  <Text color="white">{dag.metadata?.title || dag.dag_id}</Text>
                  <Text color="gray">({dag.dag_id})</Text>
                </Box>
              ))
            )}
            {data.dags.length > 12 && (
              <Text color="gray">…and {data.dags.length - 12} more</Text>
            )}
          </Box>

          <Box flexDirection="column" marginTop={1}>
            <Text bold>Recent WIHs</Text>
            {data.wihs.length === 0 ? (
              <Text color="gray">No work items found.</Text>
            ) : (
              data.wihs.slice(0, 8).map((wih, idx) => (
                <Box key={wih.wih_id || idx} gap={1}>
                  <Text color="gray">•</Text>
                  <Text color={wih.status === 'closed' || wih.status === 'completed' ? 'green' : 'white'}>
                    {wih.title || wih.wih_id}
                  </Text>
                  <Text color="gray">{wih.status}</Text>
                </Box>
              ))
            )}
          </Box>

          <Box marginTop={1}>
            <Text color="gray">Press Esc or q to close.</Text>
          </Box>
        </>
      )}
    </Box>
  )
}

export async function call(
  onDone: LocalJSXCommandOnDone,
): Promise<React.ReactNode> {
  return <OntologyViewer onDone={onDone} />
}
