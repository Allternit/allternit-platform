import { afterEach, describe, expect, it, vi } from 'vitest'
import { Editor, Node } from '@tiptap/core'
import { AgentPresence, agentPresenceKey, changedRange, showAgentEdit, CARET_LINGER_MS } from './agent-presence'

const Document = Node.create({ name: 'doc', topNode: true, content: 'block+' })
const Paragraph = Node.create({
  name: 'paragraph',
  group: 'block',
  content: 'inline*',
  parseHTML: () => [{ tag: 'p' }],
  renderHTML: () => ['p', 0],
})
const Text = Node.create({ name: 'text', group: 'inline' })

function makeEditor(html = '<p>Lead with the weight.</p>') {
  return new Editor({ element: document.createElement('div'), extensions: [Document, Paragraph, Text, AgentPresence], content: html })
}
const state = (e: Editor) => agentPresenceKey.getState(e.state)!
const advance = (e: Editor, by: number) =>
  e.view.dispatch(e.state.tr.setMeta(agentPresenceKey, { type: 'advance', by }).setMeta('addToHistory', false))

describe('agent presence', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('finds the range an edit changed', () => {
    const e = makeEditor()
    const before = e.state.doc
    e.commands.insertContentAt(e.state.doc.content.size, '<p>Say it once, then prove it.</p>')
    const r = changedRange(before, e.state.doc)!
    expect(e.state.doc.textBetween(r.from, r.to)).toBe('Say it once, then prove it.')
    expect(changedRange(e.state.doc, e.state.doc)).toBeNull()
    e.destroy()
  })

  it('reveals an agent edit behind its labelled caret, then leaves the caret at the end', () => {
    vi.stubGlobal('requestAnimationFrame', () => 1)
    vi.stubGlobal('matchMedia', () => ({ matches: false }))
    const e = makeEditor()
    const before = e.state.doc
    e.commands.insertContentAt(e.state.doc.content.size, '<p>Say it once.</p>')
    showAgentEdit(e, before)
    const s = state(e)
    expect(s.reveal).not.toBeNull()
    expect(e.view.dom.querySelector('.co-unrevealed')?.textContent).toBe('Say it once.')
    expect(e.view.dom.querySelector('.co-caret-agent .co-caret-label')?.textContent).toBe('Allternit')
    advance(e, 4)
    expect(state(e).cursor).toBe(s.reveal!.pos + 4)
    // The first step crosses into the new paragraph, then one letter per step
    expect(e.view.dom.querySelector('.co-unrevealed')?.textContent).toBe(' it once.')
    advance(e, 1000)
    expect(state(e).reveal).toBeNull()
    expect(state(e).cursor).toBe(s.reveal!.to)
    expect(state(e).until).toBeLessThanOrEqual(Date.now() + CARET_LINGER_MS)
    expect(e.view.dom.querySelector('.co-unrevealed')).toBeNull()
    // Nothing the plugin does is undoable content
    expect(e.state.doc.textContent).toBe('Lead with the weight.Say it once.')
    e.destroy()
  })

  it('shows the user’s "You" caret while the agent is present', () => {
    vi.stubGlobal('matchMedia', () => ({ matches: true }))
    const e = makeEditor()
    e.commands.setTextSelection(3)
    const before = e.state.doc
    // As the agent's tools do: the user's selection stays where it was
    e.commands.insertContentAt(e.state.doc.content.size, '<p>More.</p>', { updateSelection: false })
    showAgentEdit(e, before)
    expect(state(e).reveal).toBeNull() // reduced motion: no reveal
    expect(e.view.dom.querySelector('.co-caret-you .co-caret-label')?.textContent).toBe('You')
    e.view.dispatch(e.state.tr.setMeta(agentPresenceKey, { type: 'clear' }))
    expect(e.view.dom.querySelector('.co-caret')).toBeNull()
    e.destroy()
  })

  it('keeps its positions right when the user types before the edit', () => {
    vi.stubGlobal('requestAnimationFrame', () => 1)
    vi.stubGlobal('matchMedia', () => ({ matches: false }))
    const e = makeEditor()
    const before = e.state.doc
    e.commands.insertContentAt(e.state.doc.content.size, '<p>Tail.</p>')
    showAgentEdit(e, before)
    const { to } = state(e).reveal!
    e.commands.insertContentAt(1, 'AB')
    expect(state(e).reveal!.to).toBe(to + 2)
    e.destroy()
  })
})
