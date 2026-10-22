import { useCallback, useEffect, useRef, useState } from 'react'
import type { PointerEvent as ReactPointerEvent } from 'react'
import type { Editor } from '@tiptap/core'
import type { Block } from '@allternit/office-docx-engine'
import type { AiSettings } from '../../shared/ipc'
import { findNumId, type AiTrack, type NumIds } from './protocol'
import { createDocsSkill } from './docs-skill'
import { useI18n } from '../i18n/locale'
import { IconNewChat, IconSidebarCollapse } from '../components/icons'
import {
  useOfficeAi,
  useOfficeHost,
  registerPaneAgent,
  paneAgentAuthor,
  AllternitBrandMark,
  type OfficeAgentLoop,
} from '@allternit/office-suite/bridge'
import { type OfficeModelOption } from '@allternit/office-suite/ai'

const PANEL_WIDTH_KEY = 'docs-ai-panel-width'
const PANEL_WIDTH_DEFAULT = 360
const PANEL_WIDTH_MIN = 280

function clampPanelWidth(w: number): number {
  return Math.min(Math.max(w, PANEL_WIDTH_MIN), Math.min(720, Math.round(window.innerWidth * 0.6)))
}

function loadPanelWidth(): number {
  const saved = Number(localStorage.getItem(PANEL_WIDTH_KEY))
  return Number.isFinite(saved) && saved > 0 ? clampPanelWidth(saved) : PANEL_WIDTH_DEFAULT
}

interface AiPanelProps {
  editor: Editor
  blocks: Block[]
  settings: AiSettings
  /** the document has no text yet — the empty-state copy offers drafting instead of editing */
  docEmpty?: boolean
  /** fallback numbering ids for documents created from the blank template */
  numIdFallback?: NumIds | null
  /** preset instruction pushed from the ribbon or start screen; autoRun sends it immediately */
  preset?: { text: string; nonce: number; autoRun?: boolean } | null
  /** false shows only the collapsed rail; the component stays mounted so panel state survives */
  open?: boolean
  /** expand from the collapsed rail */
  onExpand?: () => void
  /** collapse the panel to the sidebar rail */
  onCollapse?: () => void
  /** Absolute path of the currently open file (used for chat-history persistence) */
  filePath?: string | null
}

interface ChatEntry {
  role: 'user' | 'assistant'
  text: string
  streaming?: boolean
}

/**
 * AI assistant for Allternit Docs — a chat panel streaming the platform
 * agent-chat endpoint via @allternit/office-ai, running the docx skill
 * (docs-skill.ts: document context + the document tools, executed against
 * the live editor). The same skill is registered for a host chat
 * (registerPaneAgent), so an artifact session's agent edits this document.
 */
const APP_KEY = 'docs' as const

export function AiPanel({ editor, blocks, docEmpty, numIdFallback, preset, open, onExpand, onCollapse }: AiPanelProps) {
  const { t } = useI18n()
  const ai = useOfficeAi()
  // With a host chat the panel is mounted hidden only to own the skill.
  const hostChat = useOfficeHost()?.hostChat
  const [chat, setChat] = useState<ChatEntry[]>([])
  const [prompt, setPrompt] = useState('')
  const [busy, setBusy] = useState(false)
  const [panelWidth, setPanelWidth] = useState(loadPanelWidth)
  const [modelId, setModelId] = useState<string | undefined>(() => ai.resolveModelId(APP_KEY))
  const [resizing, setResizing] = useState(false)
  const panelRef = useRef<HTMLDivElement>(null)
  const resizeCleanupRef = useRef<(() => void) | null>(null)
  const lastPresetNonce = useRef<number | null>(null)

  // The .ai-dock wrapper owns the animated width; sync the resizable panel width.
  useEffect(() => {
    const dock = panelRef.current?.closest('.ai-dock') as HTMLElement | null
    dock?.style.setProperty('--ai-panel-width', `${panelWidth}px`)
  }, [panelWidth])

  // Re-clamp the persisted width when the window shrinks.
  useEffect(() => {
    const onResize = (): void => setPanelWidth((w) => clampPanelWidth(w))
    window.addEventListener('resize', onResize)
    return () => window.removeEventListener('resize', onResize)
  }, [])

  useEffect(() => () => resizeCleanupRef.current?.(), [])

  /** Drag the right edge to resize: the panel is flush with the window's left edge, so width = clientX */
  const startResize = (e: ReactPointerEvent<HTMLDivElement>): void => {
    e.preventDefault()
    const resizer = e.currentTarget
    setResizing(true)
    document.body.style.cursor = 'col-resize'
    document.body.style.userSelect = 'none'
    const onMove = (ev: PointerEvent): void => {
      setPanelWidth(clampPanelWidth(ev.clientX))
    }
    let done = false
    const cleanup = (): void => {
      if (done) return
      done = true
      resizeCleanupRef.current = null
      window.removeEventListener('pointermove', onMove)
      window.removeEventListener('pointerup', cleanup)
      window.removeEventListener('pointercancel', cleanup)
      resizer.removeEventListener('lostpointercapture', cleanup)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
      setResizing(false)
      setPanelWidth((w) => {
        localStorage.setItem(PANEL_WIDTH_KEY, String(Math.round(w)))
        return w
      })
    }
    resizeCleanupRef.current = cleanup
    window.addEventListener('pointermove', onMove)
    window.addEventListener('pointerup', cleanup)
    window.addEventListener('pointercancel', cleanup)
    resizer.addEventListener('lostpointercapture', cleanup)
    resizer.setPointerCapture(e.pointerId)
  }

  const loopRef = useRef<OfficeAgentLoop | null>(null)
  // The loop and skill are constructed once, so they read the editor, blocks
  // and numbering fallback through refs to see the current document.
  const blocksRef = useRef(blocks)
  blocksRef.current = blocks
  const editorRef = useRef(editor)
  editorRef.current = editor
  const numIdFallbackRef = useRef(numIdFallback)
  numIdFallbackRef.current = numIdFallback
  const skillRef = useRef<ReturnType<typeof createDocsSkill> | null>(null)
  if (!skillRef.current) {
    const numIds = (): NumIds => ({
      bullet: findNumId(blocksRef.current ?? [], 'bullet') ?? numIdFallbackRef.current?.bullet ?? null,
      ordered: findNumId(blocksRef.current ?? [], 'ordered') ?? numIdFallbackRef.current?.ordered ?? null,
    })
    // A host agent's call (an Allternit session agent through the pane bridge)
    // edits as tracked changes under its name; this panel's own replies stay direct.
    const track = (): AiTrack | undefined => {
      const author = paneAgentAuthor(APP_KEY)
      return author ? { author } : undefined
    }
    skillRef.current = createDocsSkill(() => editorRef.current, numIds, track)
  }
  useEffect(() => registerPaneAgent(APP_KEY, skillRef.current!), [])
  if (!loopRef.current) {
    loopRef.current = new ai.AgentLoop({
      modelId,
      skill: skillRef.current,
      // Drafting a document takes several tool rounds; the default 8 cuts it off.
      maxTurns: 24,
      events: {
        onText: (text) => {
          setChat((previous) => {
            const next = [...previous]
            const last = next[next.length - 1]
            if (last?.role === 'assistant') {
              next[next.length - 1] = { ...last, text, streaming: false }
            } else {
              next.push({ role: 'assistant', text })
            }
            return next
          })
        },
        onDone: () => setBusy(false),
        onError: (error) => {
          setBusy(false)
          setChat((previous) => [
            ...previous,
            { role: 'assistant', text: `⚠ ${error}` },
          ])
        },
      },
    })
  }

  const run = useCallback(
    (instruction: string) => {
      const trimmed = instruction.trim()
      if (!trimmed || busy) return
      setChat((previous) => [...previous, { role: 'user', text: trimmed }])
      setPrompt('')
      setBusy(true)
      loopRef.current!.run(trimmed)
    },
    [busy],
  )

  // Ribbon/start-screen presets: run once per nonce. With a host chat the
  // host runs them (onAssistantPreset), never this hidden panel.
  if (preset && !hostChat && preset.nonce !== lastPresetNonce.current) {
    lastPresetNonce.current = preset.nonce
    if (preset.autoRun) run(preset.text)
    else setPrompt(preset.text)
  }

  if (!open) {
    return (
      <button className="ai-rail" title={t('appExpandAiPanel')} onClick={onExpand}>
        <span className="ai-rail-text">AI</span>
      </button>
    )
  }

  return (
    <div ref={panelRef} className={`ai-panel${resizing ? ' ai-panel-resizing' : ''}`}>
      <div
        className="ai-panel-resizer"
        onPointerDown={startResize}
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize AI panel"
      />
      <header className="ai-panel-header">
        <span className="ai-panel-title">
          <AllternitBrandMark size={16} />
          Allternit Office Agent
        </span>
        <div className="ai-panel-header-actions">
          <ModelPicker
            value={modelId}
            onChange={(next) => {
              setModelId(next)
              ai.setModelOverride(APP_KEY, next)
              loopRef.current?.setModelId(next)
            }}
          />
          {chat.length > 0 && (
            <button
              className="ai-header-btn"
              onClick={() => {
                loopRef.current?.reset()
                setChat([])
                setPrompt('')
              }}
              title={t('aiNewChatTitle')}
            >
              <IconNewChat size={15} />
            </button>
          )}
          <button className="ai-header-btn" onClick={onCollapse} title={t('aiCollapseTitle')}>
            <IconSidebarCollapse size={15} />
          </button>
        </div>
      </header>
      <div className="ai-chat">
        {chat.length === 0 ? (
          <div className="ai-chat-empty">
            <div className="ai-chat-empty-title">
              {docEmpty ? 'Draft a document' : 'Ask about this document'}
            </div>
            <div className="ai-chat-empty-body">
              {docEmpty
                ? 'Describe the document you want — the assistant will draft it with your document as context.'
                : 'Summarize, rewrite, or extend the open document.'}
            </div>
          </div>
        ) : (
          chat.map((entry, index) => (
            <div key={index} className={`ai-msg ai-msg-${entry.role}`}>
              <div className="ai-markdown" style={{ whiteSpace: 'pre-wrap' }}>
                {entry.text}
                {entry.streaming ? '…' : ''}
              </div>
            </div>
          ))
        )}
        {busy ? (
          <div className="ai-typing-row">
            <span className="ai-typing">
              <span className="ai-typing-dots">
                <span className="ai-typing-dot-slot">
                  <span className="ai-typing-dot-grow">●</span>
                </span>
              </span>
              <span className="ai-typing-label">Thinking…</span>
            </span>
          </div>
        ) : null}
      </div>
      <div className="ai-composer">
        <div className="ai-input-box">
          <textarea
            value={prompt}
            placeholder={docEmpty ? 'Describe the document to draft…' : 'Ask about this document…'}
            aria-label="AI instruction"
            rows={2}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault()
                run(prompt)
              }
            }}
          />
          <div className="ai-input-footer">
            <span className="ai-input-hint">Enter to send · Shift+Enter for newline</span>
            {busy ? (
              <button
                className="ai-send-btn ai-stop-btn"
                aria-label="Stop"
                onClick={() => loopRef.current?.cancel()}
              >
                ■
              </button>
            ) : (
              <button
                className="ai-send-btn"
                aria-label="Send"
                disabled={!prompt.trim()}
                onClick={() => run(prompt)}
              >
                ↵
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  )
}

function ModelPicker({
  value,
  onChange,
}: {
  value?: string | undefined
  onChange?: (modelId: string | undefined) => void
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const ai = useOfficeAi()
  const [options, setOptions] = useState<OfficeModelOption[]>(() => ai.getModelOptions())
  const selected = options.find((o) => o.id === (value ?? 'platform')) ?? options[0]
  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    let cancelled = false
    ai.refreshModelOptions()
      .then((next) => {
        if (!cancelled) setOptions(next)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    if (!open) return
    const onDocClick = (e: MouseEvent): void => {
      if (!menuRef.current?.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', onDocClick)
    return () => document.removeEventListener('mousedown', onDocClick)
  }, [open])

  return (
    <div className="ai-model-picker" ref={menuRef}>
      <button
        type="button"
        className="ai-model-picker-trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        title={selected?.label}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="ai-model-picker-label">{selected?.label}</span>
        <span className="ai-model-picker-caret" aria-hidden>
          ▾
        </span>
      </button>
      {open && (
        <div className="ai-model-picker-menu" role="listbox">
          {options.map((o) => (
            <button
              key={o.id}
              type="button"
              role="option"
              aria-selected={o.id === selected?.id}
              className={`ai-model-picker-option${o.id === selected?.id ? ' active' : ''}`}
              onClick={() => {
                onChange?.(o.id === 'platform' ? undefined : o.id)
                setOpen(false)
              }}
            >
              <span className="ai-model-picker-option-label">{o.label}</span>
              {o.provider && (
                <span className="ai-model-picker-option-provider">{o.provider}</span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
