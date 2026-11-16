import { useEffect, useMemo, useState, type CSSProperties } from 'react'
import {
  AlertTriangle,
  ArrowUpRight,
  FileSpreadsheet,
  FileText,
  MonitorSmartphone,
  Presentation,
} from 'lucide-react'
import { getBridge } from '@/lib/bridge-factory'
import { getOfficeHost, getOfficeHostDisplayName, getOfficeManifestUrl, getOfficeProductTarget } from '@/lib/host-detector'
import {
  convertBytesToMarkdown,
  filenameForConversion,
  readCurrentDocumentBytes,
  type MarkdownConversionResult,
} from '@/lib/markdown-conversion'
import {
  bootstrapOfficeRuntime,
  getOfficeBootstrapState,
  getPlatformOrigin,
  OFFICE_BOOTSTRAP_UPDATED_EVENT,
  resolveOfficeDocumentSnapshot,
  setAuthToken,
  syncOfficeRuntimeState,
  type OfficeBindingSnapshot,
} from '@/lib/platform-gateway'
import OfficeSidepanelApp from './OfficeSidepanelApp'
import { isOfficeRuntimeReady, resolveTaskpaneMode, type TaskpaneRuntimeMode } from './runtime-mode'
import { AProtocolMark } from './components/AProtocolMark'

type BridgeStatus = 'connecting' | 'connected' | 'error' | 'companion'

type MarkdownPanelState =
  | { kind: 'idle' }
  | { kind: 'loading' }
  | { kind: 'ready'; result: MarkdownConversionResult }
  | { kind: 'error'; message: string }

const HOST_ACCENTS = {
  word: '#2B579A',
  excel: '#217346',
  powerpoint: '#D74726',
  unknown: '#1F1E1D',
} as const

type HostKey = keyof typeof HOST_ACCENTS

const HOST_PRODUCTS = {
  word: {
    role: 'Writing and review',
    description: 'Draft, revise, research, and review the active document.',
    actions: ['Review this document', 'Rewrite the selection', 'Create an outline', 'Check citations'],
  },
  excel: {
    role: 'Analysis and modeling',
    description: 'Inspect formulas, clean data, model scenarios, and explain the workbook.',
    actions: ['Analyze this workbook', 'Audit formulas', 'Clean the selected range', 'Build a chart'],
  },
  powerpoint: {
    role: 'Narrative and slide design',
    description: 'Strengthen the story, revise slides, create notes, and improve consistency.',
    actions: ['Review the deck story', 'Rewrite this slide', 'Create speaker notes', 'Check consistency'],
  },
  unknown: {
    role: 'Office developer preview',
    description: 'Open this product from its matching Microsoft Office host.',
    actions: [],
  },
} as const

const HOST_ICONS: Record<HostKey, typeof FileText> = {
  word: FileText,
  excel: FileSpreadsheet,
  powerpoint: Presentation,
  unknown: FileText,
}

const STATUS_META: Record<BridgeStatus, { label: string; dotClass: string; pulse: boolean }> = {
  connected: { label: 'Connected', dotClass: 'bg-[var(--status-success)]', pulse: false },
  connecting: { label: 'Connecting', dotClass: 'bg-[var(--status-warning)]', pulse: true },
  companion: { label: 'No Office host', dotClass: 'bg-[var(--text-tertiary)]', pulse: false },
  error: { label: 'Reconnect needed', dotClass: 'bg-[var(--status-error)]', pulse: false },
}

export default function App() {
  const [mode, setMode] = useState<TaskpaneRuntimeMode>(() =>
    resolveTaskpaneMode({ officeInitialized: isOfficeRuntimeReady(), bootstrap: getOfficeBootstrapState() }),
  )

  useEffect(() => {
    const recompute = () =>
      setMode(resolveTaskpaneMode({ officeInitialized: isOfficeRuntimeReady(), bootstrap: getOfficeBootstrapState() }))
    // Office.onReady fired late, or the shell pushed a bootstrap context
    // (token / workspace / project) after first render.
    window.addEventListener('allternit-office-runtime-ready', recompute)
    window.addEventListener('allternit-office-auth-token-received', recompute)
    window.addEventListener(OFFICE_BOOTSTRAP_UPDATED_EVENT, recompute)
    return () => {
      window.removeEventListener('allternit-office-runtime-ready', recompute)
      window.removeEventListener('allternit-office-auth-token-received', recompute)
      window.removeEventListener(OFFICE_BOOTSTRAP_UPDATED_EVENT, recompute)
    }
  }, [])

  if (mode === 'full-ai') return <OfficeSidepanelApp />
  return <CompanionApp />
}

function CompanionApp() {
  const liveHost = getOfficeHost()
  const host = getOfficeProductTarget()
  const hostLabel = getOfficeHostDisplayName()
  const accent = HOST_ACCENTS[host]
  const [status, setStatus] = useState<BridgeStatus>(liveHost === 'unknown' ? 'companion' : 'connecting')
  const [binding, setBinding] = useState<OfficeBindingSnapshot | null>(null)
  const [documentLabel, setDocumentLabel] = useState(`${hostLabel} document`)
  const [error, setError] = useState<string | null>(null)
  const [connectionAttempt, setConnectionAttempt] = useState(0)
  const [markdownPanel, setMarkdownPanel] = useState<MarkdownPanelState>({ kind: 'idle' })
  const platformOrigin = useMemo(() => getPlatformOrigin(), [])
  const product = HOST_PRODUCTS[host]
  const HostIcon = HOST_ICONS[host]
  const statusMeta = STATUS_META[status]

  const viewAsMarkdown = async () => {
    setMarkdownPanel({ kind: 'loading' })
    try {
      const bytes = await readCurrentDocumentBytes()
      const documentUrl = typeof Office !== 'undefined' ? Office.context?.document?.url : undefined
      const result = await convertBytesToMarkdown(bytes, filenameForConversion(host, documentUrl))
      setMarkdownPanel({ kind: 'ready', result })
    } catch (reason) {
      setMarkdownPanel({ kind: 'error', message: reason instanceof Error ? reason.message : String(reason) })
    }
  }

  const connectAllternit = () => {
    const authUrl = `${platformOrigin}/office-auth-bridge`
    const officeUi = typeof Office !== 'undefined' ? Office.context?.ui : undefined

    if (officeUi?.displayDialogAsync) {
      officeUi.displayDialogAsync(authUrl, { height: 65, width: 35, displayInIframe: false }, (result) => {
        if (result.status !== Office.AsyncResultStatus.Succeeded || !result.value) {
          setError(result.error?.message ?? 'Could not open Allternit sign in.')
          return
        }

        const dialog = result.value
        dialog.addEventHandler(Office.EventType.DialogMessageReceived, (event) => {
          try {
            if (!('message' in event)) throw new Error('The sign-in dialog did not return a message.')
            const payload = JSON.parse(event.message ?? '{}') as { token?: string }
            if (!payload.token) throw new Error('No authentication token was returned.')
            setAuthToken(payload.token)
            dialog.close()
            setStatus('connecting')
            setError(null)
            setConnectionAttempt((attempt) => attempt + 1)
          } catch (reason) {
            setError(reason instanceof Error ? reason.message : 'Could not finish Allternit sign in.')
          }
        })
        dialog.addEventHandler(Office.EventType.DialogEventReceived, () => dialog.close())
      })
      return
    }

    const popup = window.open(authUrl, 'allternit-office-auth', 'popup=yes,width=520,height=720')
    if (!popup) setError('Allow popups for this add-in, then try connecting again.')
  }

  const steerAgent = (instruction: string) => {
    if (window.parent === window) {
      window.open(`${platformOrigin}/?officeHost=${host}&prompt=${encodeURIComponent(instruction)}`, '_blank', 'noopener,noreferrer')
      return
    }
    window.parent.postMessage({ source: 'allternit-office-addin', type: 'steer-agent', payload: { host, instruction, bindingId: binding?.id ?? null } }, '*')
  }

  useEffect(() => {
    if (liveHost === 'unknown') return
    let cancelled = false
    let heartbeat: number | undefined

    const connect = async () => {
      try {
        const context = await getBridge().getContext()
        if (cancelled) return
        setDocumentLabel(context.label)
        const document = await resolveOfficeDocumentSnapshot(context)
        const bootstrap = getOfficeBootstrapState()
        const result = await bootstrapOfficeRuntime({
          document,
          platform: {
            taskpane_origin: window.location.origin,
            taskpane_url: window.location.href,
            manifest_url: getOfficeManifestUrl(),
            platform_origin: platformOrigin,
          },
          runtimeState: {
            status: 'bridge-ready',
            page_label: context.label,
            connected: true,
          },
          workspaceId: bootstrap.context.workspaceId,
          projectId: bootstrap.context.projectId,
        })
        if (cancelled) return
        setBinding(result.binding)
        setStatus('connected')
        setError(null)
        heartbeat = window.setInterval(() => {
          void syncOfficeRuntimeState({
            bindingId: result.binding.id,
            runtimeState: { status: 'bridge-ready', page_label: context.label, connected: true },
            workspaceId: bootstrap.context.workspaceId,
            projectId: bootstrap.context.projectId,
          }).catch(() => setStatus('error'))
        }, 10_000)
      } catch (reason) {
        if (cancelled) return
        setStatus('error')
        setError(reason instanceof Error ? reason.message : String(reason))
      }
    }

    void connect()
    return () => {
      cancelled = true
      if (heartbeat) window.clearInterval(heartbeat)
    }
  }, [connectionAttempt, host, liveHost, platformOrigin])

  useEffect(() => {
    const reconnect = () => {
      setStatus('connecting')
      setError(null)
      setConnectionAttempt((attempt) => attempt + 1)
    }
    window.addEventListener('allternit-office-auth-token-received', reconnect)
    return () => window.removeEventListener('allternit-office-auth-token-received', reconnect)
  }, [])

  return (
    <main
      className="flex h-full min-h-0 flex-col bg-[var(--bg-primary)] text-[var(--text-primary)]"
      style={{ '--host-accent': accent } as CSSProperties}
    >
      {/* ── Header ── */}
      <header className="flex shrink-0 items-center gap-2 border-b border-[var(--border-subtle)] bg-[var(--bg-elevated)] px-3 py-2.5">
        <AProtocolMark height={12} suffix="OFFICE" className="shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-[12px] font-bold leading-tight">{hostLabel}</div>
        </div>
        <span className="flex shrink-0 items-center gap-1 rounded-full border border-[var(--border-subtle)] bg-[var(--bg-primary)] px-1.5 py-0.5 text-[9px] font-semibold text-[var(--text-secondary)]">
          <span
            className={`status-dot size-1.5 ${statusMeta.dotClass} ${statusMeta.pulse ? 'is-pulsing' : ''}`}
          />
          {statusMeta.label}
        </span>
      </header>

      {/* ── Body ── */}
      <section className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4">
        {status === 'connecting' && (
          <div className="card host-border space-y-2.5 p-4" aria-busy="true">
            <div className="flex items-center gap-2">
              <span className="host-tint-strong flex size-8 items-center justify-center rounded-lg">
                <HostIcon className="size-4 host-accent" strokeWidth={1.8} />
              </span>
              <div className="flex-1 space-y-1.5">
                <div className="shimmer h-3 w-3/4" />
                <div className="shimmer h-2.5 w-1/3" />
              </div>
            </div>
            <div className="shimmer h-2.5 w-full" />
            <p className="pt-1 text-[11px] leading-relaxed text-[var(--text-tertiary)]">
              Attaching to the open {hostLabel} document…
            </p>
          </div>
        )}

        {(status === 'connected' || status === 'error') && (
          <div className="card host-border p-4">
            <div className="eyebrow mb-2.5">Attached document</div>
            <div className="flex items-start gap-3">
              <span className="host-tint-strong mt-0.5 flex size-9 shrink-0 items-center justify-center rounded-lg">
                <HostIcon className="size-[18px] host-accent" strokeWidth={1.8} />
              </span>
              <div className="min-w-0 flex-1">
                <div className="break-words text-[13px] font-semibold leading-snug">{documentLabel}</div>
                <div className="mt-1.5 flex flex-wrap gap-1.5">
                  <span className="chip">{hostLabel}</span>
                  {binding?.workspace_id && <span className="chip">Workspace attached</span>}
                  {binding?.project_id && <span className="chip">Project attached</span>}
                </div>
              </div>
            </div>

            {status === 'connected' && markdownPanel.kind === 'idle' && (
              <button
                type="button"
                onClick={() => void viewAsMarkdown()}
                className="btn btn-outline mt-3.5 h-8 px-3"
                data-testid="view-as-markdown"
              >
                View as Markdown
              </button>
            )}
            {markdownPanel.kind === 'loading' && (
              <div className="mt-3.5 flex items-center gap-2 text-[11px] text-[var(--text-tertiary)]">
                <span className="status-dot size-1.5 bg-[var(--status-warning)] is-pulsing" />
                Converting to Markdown…
              </div>
            )}
            {markdownPanel.kind === 'error' && (
              <div className="notice notice-error mt-3.5 flex-col">
                <div>{markdownPanel.message}</div>
                <button
                  type="button"
                  onClick={() => void viewAsMarkdown()}
                  className="btn btn-primary mt-1 h-7 self-start px-3 text-[11px]"
                >
                  Retry
                </button>
              </div>
            )}
            {markdownPanel.kind === 'ready' && (
              <div className="mt-3.5" data-testid="markdown-panel">
                <div className="mb-1.5 flex items-center justify-between gap-2">
                  <span className="chip uppercase">
                    {markdownPanel.result.format ?? 'markdown'}
                  </span>
                  <div className="flex items-center gap-2.5">
                    <a
                      href={`${platformOrigin}/markdown-preview`}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="flex items-center gap-0.5 text-[11px] font-semibold text-[var(--accent-brand)] hover:underline"
                    >
                      Open in platform
                      <ArrowUpRight className="size-3" />
                    </a>
                    <button
                      type="button"
                      onClick={() => setMarkdownPanel({ kind: 'idle' })}
                      className="text-[11px] font-medium text-[var(--text-tertiary)] transition-colors hover:text-[var(--text-primary)]"
                    >
                      Close
                    </button>
                  </div>
                </div>
                <pre className="max-h-72 overflow-auto whitespace-pre-wrap break-words rounded-[var(--radius-lg)] border border-[var(--border-subtle)] bg-[var(--bg-primary)] p-3 font-mono text-[11px] leading-relaxed text-[var(--text-secondary)]">
                  {markdownPanel.result.markdown}
                </pre>
              </div>
            )}
          </div>
        )}

        {/* ── What the agent can do here ── */}
        <div className="host-tint host-border rounded-[var(--radius-xl)] border p-4">
          <div className="flex items-center gap-2 text-[12px] font-bold">
            <span className="host-accent font-mono text-[10px] font-bold">A://</span>
            {product.role}
          </div>
          <p className="mt-1.5 text-[11.5px] leading-relaxed text-[var(--text-secondary)]">
            {product.description} The platform brain owns models, skills, approvals, memory, and execution history.
          </p>
        </div>

        {/* ── Suggested actions ── */}
        {product.actions.length > 0 && (
          <div>
            <div className="eyebrow mb-2">Start with Computer Agent</div>
            <div className="grid grid-cols-2 gap-2">
              {product.actions.map((action) => (
                <button
                  key={action}
                  type="button"
                  disabled={status !== 'connected'}
                  onClick={() => steerAgent(action)}
                  className="card group flex min-h-14 items-center justify-between gap-2 p-3 text-left transition-all hover:shadow-[var(--shadow-md)] disabled:cursor-not-allowed disabled:opacity-45"
                >
                  <span className="text-[11.5px] font-semibold leading-4">{action}</span>
                  <ArrowUpRight className="size-3.5 shrink-0 text-[var(--text-tertiary)] transition-colors group-hover-action" strokeWidth={2} />
                </button>
              ))}
            </div>
          </div>
        )}

        {/* ── States ── */}
        {status === 'companion' && (
          <div className="notice notice-warning">
            <MonitorSmartphone className="mt-0.5 size-3.5 shrink-0 text-[var(--status-warning)]" strokeWidth={1.8} />
            <span>
              Open this add-in from Word, Excel, or PowerPoint. Loading its webpage by itself cannot
              provide Office document access.
            </span>
          </div>
        )}
        {error && (
          <div className="notice notice-error flex-col">
            <div className="flex gap-2">
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0" strokeWidth={1.8} />
              <span>{error}</span>
            </div>
            {liveHost !== 'unknown' && (
              <button
                type="button"
                onClick={connectAllternit}
                className="btn btn-primary mt-1.5 h-8 self-start px-3.5 text-[11.5px]"
              >
                Connect Allternit
              </button>
            )}
          </div>
        )}
        <footer className="mt-auto pt-1 text-[10px] leading-relaxed text-[var(--text-tertiary)]">
          Platform brain · Browser/computer-use harness ·{' '}
          {binding ? `Binding ${binding.id.slice(0, 8)}` : 'Waiting for binding'}
        </footer>
      </section>
    </main>
  )
}
