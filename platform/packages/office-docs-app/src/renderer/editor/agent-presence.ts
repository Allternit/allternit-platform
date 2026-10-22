/**
 * Co-editing presence in the document: the agent's labelled cursor and its
 * edits "typed" in, and the user's own labelled cursor beside it.
 *
 * The agent's tools change the document at once (undo, save and the tools'
 * own semantics stay as they are); this plugin only reveals the changed range
 * at typing speed — the text not yet reached is transparent, a caret labelled
 * with the agent's name walks through it — then leaves the caret at the end of
 * the edit for a few seconds. While the agent is present, the user's caret
 * carries a "You" label.
 *
 * Reduced motion: no reveal, the caret just appears at the end of the edit.
 */
import { Extension, type Editor } from '@tiptap/core'
import { Plugin, PluginKey, TextSelection, type EditorState, type Transaction } from '@tiptap/pm/state'
import { Decoration, DecorationSet } from '@tiptap/pm/view'
import type { Node as PMNode } from '@tiptap/pm/model'

export interface AgentPresenceState {
  /** The part of the edit still hidden: [pos, to), uncovered at `speed` positions per ms. */
  reveal: { pos: number; to: number; speed: number } | null
  /** Where the agent's caret is (null: not shown). */
  cursor: number | null
  /** When the caret goes away (ms since epoch). */
  until: number
  label: string
}

type Meta =
  | { type: 'edit'; from: number; to: number; label?: string; animate: boolean }
  | { type: 'advance'; by: number }
  | { type: 'clear' }

export const agentPresenceKey = new PluginKey<AgentPresenceState>('agentPresence')

/** How long the agent's caret stays after it finishes (ms). */
export const CARET_LINGER_MS = 6000
/** Typing speed of the reveal (document positions per second), and its bounds in time. */
const CHARS_PER_SEC = 90
const MIN_MS = 500
const MAX_MS = 4500

const EMPTY: AgentPresenceState = { reveal: null, cursor: null, until: 0, label: 'Allternit' }

function caret(label: string, kind: 'agent' | 'you'): HTMLElement {
  const el = document.createElement('span')
  el.className = `co-caret co-caret-${kind}`
  el.setAttribute('aria-hidden', 'true')
  const tag = document.createElement('span')
  tag.className = 'co-caret-label'
  tag.textContent = label
  el.appendChild(tag)
  return el
}

/** The range two documents differ over, in `next`'s positions; null when equal. */
export function changedRange(prev: PMNode, next: PMNode): { from: number; to: number } | null {
  const start = prev.content.findDiffStart(next.content)
  if (start == null) return null
  const end = prev.content.findDiffEnd(next.content)
  if (!end) return { from: start, to: next.content.size }
  // An insertion that ends like the text before it (a ".", a "</p>") lets the start and
  // end matches overlap; push the end back out by the overlap (as prosemirror-collab does).
  let endB = end.b
  const overlap = start - Math.min(end.a, end.b)
  if (overlap > 0) endB += overlap
  return { from: start, to: Math.min(Math.max(endB, start), next.content.size) }
}

function apply(tr: Transaction, value: AgentPresenceState): AgentPresenceState {
  const meta = tr.getMeta(agentPresenceKey) as Meta | undefined
  let next = value
  if (tr.docChanged && (value.reveal || value.cursor != null)) {
    const reveal = value.reveal
      ? { ...value.reveal, pos: tr.mapping.map(value.reveal.pos, -1), to: tr.mapping.map(value.reveal.to, 1) }
      : null
    next = {
      ...value,
      reveal: reveal && reveal.pos < reveal.to ? reveal : null,
      cursor: value.cursor == null ? null : tr.mapping.map(value.cursor, 1),
    }
  }
  if (!meta) return next
  if (meta.type === 'clear') return { ...EMPTY, label: next.label }
  if (meta.type === 'edit') {
    const label = meta.label ?? next.label
    if (!meta.animate || meta.to <= meta.from) {
      return { reveal: null, cursor: meta.to, until: Date.now() + CARET_LINGER_MS, label }
    }
    // Typing speed: CHARS_PER_SEC, but the whole reveal takes MIN_MS..MAX_MS
    const size = meta.to - meta.from
    const ms = Math.min(MAX_MS, Math.max(MIN_MS, (size / CHARS_PER_SEC) * 1000))
    return { reveal: { pos: meta.from, to: meta.to, speed: size / ms }, cursor: meta.from, until: Number.POSITIVE_INFINITY, label }
  }
  if (meta.type === 'advance' && next.reveal) {
    const pos = Math.min(next.reveal.to, next.reveal.pos + meta.by)
    if (pos >= next.reveal.to) return { ...next, reveal: null, cursor: next.reveal.to, until: Date.now() + CARET_LINGER_MS }
    return { ...next, reveal: { ...next.reveal, pos }, cursor: pos }
  }
  return next
}

function decorations(state: EditorState): DecorationSet {
  const s = agentPresenceKey.getState(state)
  if (!s || (s.cursor == null && !s.reveal)) return DecorationSet.empty
  const decos: Decoration[] = []
  const size = state.doc.content.size
  if (s.reveal) {
    const { pos, to } = s.reveal
    decos.push(Decoration.inline(pos, to, { class: 'co-unrevealed' }))
    // Blocks (charts, images, tables) wholly ahead of the caret stay hidden too.
    state.doc.nodesBetween(pos, to, (node, at) => {
      if (at >= pos && at + node.nodeSize <= to && (node.isAtom || node.type.name === 'table')) {
        decos.push(Decoration.node(at, at + node.nodeSize, { class: 'co-unrevealed-block' }))
        return false
      }
      return true
    })
  }
  if (s.cursor != null) {
    // Carets sit in text: an edit that ends on a block boundary puts it at the end of
    // the text before (not on an empty line of its own between blocks).
    const clamped = Math.max(0, Math.min(size, s.cursor))
    const at = state.doc.resolve(clamped).parent.inlineContent
      ? clamped
      : TextSelection.near(state.doc.resolve(clamped), -1).head
    decos.push(Decoration.widget(at, () => caret(s.label, 'agent'), { side: 1, key: `agent-${s.label}` }))
    // The user's own caret, labelled, while the agent is in the document with them.
    const head = state.selection.head
    if (state.selection.empty && head !== at) {
      decos.push(Decoration.widget(head, () => caret('You', 'you'), { side: -1, key: 'you' }))
    }
  }
  return DecorationSet.create(state.doc, decos)
}

export const AgentPresence = Extension.create({
  name: 'agentPresence',
  addProseMirrorPlugins() {
    return [
      new Plugin<AgentPresenceState>({
        key: agentPresenceKey,
        state: { init: () => EMPTY, apply },
        props: { decorations },
        view: (view) => {
          let raf = 0
          let last = 0
          let timer: ReturnType<typeof setTimeout> | null = null
          const tick = (now: number) => {
            raf = 0
            const s = agentPresenceKey.getState(view.state)
            if (!s?.reveal) return
            const elapsed = last ? now - last : 33
            // ~30 steps a second is plenty for typing and keeps big documents smooth
            if (last && elapsed < 30) {
              raf = requestAnimationFrame(tick)
              return
            }
            last = now
            const by = Math.max(1, Math.round(s.reveal.speed * elapsed))
            // Re-arm before dispatching: the dispatch runs update → schedule, which must see
            // this loop still running (else every step starts another loop and they race)
            raf = requestAnimationFrame(tick)
            view.dispatch(view.state.tr.setMeta(agentPresenceKey, { type: 'advance', by }).setMeta('addToHistory', false))
          }
          const schedule = () => {
            const s = agentPresenceKey.getState(view.state)
            if (s?.reveal && !raf) {
              last = 0
              raf = requestAnimationFrame(tick)
            }
            if (timer) clearTimeout(timer)
            timer = null
            if (s && !s.reveal && s.cursor != null && Number.isFinite(s.until)) {
              timer = setTimeout(() => {
                const cur = agentPresenceKey.getState(view.state)
                if (cur && !cur.reveal && cur.until <= Date.now() + 5) {
                  view.dispatch(view.state.tr.setMeta(agentPresenceKey, { type: 'clear' }).setMeta('addToHistory', false))
                }
              }, Math.max(0, s.until - Date.now()))
            }
          }
          schedule()
          return {
            update: schedule,
            destroy: () => {
              if (raf) cancelAnimationFrame(raf)
              if (timer) clearTimeout(timer)
            },
          }
        },
      }),
    ]
  },
})

function reducedMotion(): boolean {
  return typeof window !== 'undefined' && !!window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
}

/** Show an agent edit: `prev` is the document before the agent's change. */
export function showAgentEdit(editor: Editor, prev: PMNode, label?: string): void {
  if (editor.isDestroyed || !editor.state.plugins.some((p) => p.spec.key === agentPresenceKey)) return
  const range = changedRange(prev, editor.state.doc)
  if (!range) return
  editor.view.dispatch(
    editor.state.tr
      .setMeta(agentPresenceKey, { type: 'edit', from: range.from, to: range.to, label, animate: !reducedMotion() })
      .setMeta('addToHistory', false),
  )
}
