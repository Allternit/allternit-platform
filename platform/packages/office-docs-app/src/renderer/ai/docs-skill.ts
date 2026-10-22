import type { Editor } from '@tiptap/core'
import type { AgentToolCall, AgentToolDef } from '../../shared/ipc'
import { AGENT_SYSTEM_PROMPT, buildDocContext, type AiTrack, type NumIds } from './protocol'
import { AGENT_TOOLS, executeTool, markDocSeen } from './tools'
import { showAgentEdit } from '../editor/agent-presence'

/** Same shape as @genoffice/agent-core's AgentSkill (what OfficeAgentLoop consumes). */
export interface AgentSkill {
  id: string
  systemPrompt: string
  tools: AgentToolDef[]
  buildContext: () => string
  executeTool: (call: AgentToolCall, signal?: AbortSignal) => unknown
  /** The selected text (or the block the cursor is in), for anchoring a comment. */
  anchor?: () => string | null
}

/**
 * The docx capability as an AgentSkill: document skeleton context, the five
 * document tools, and the local executor. Future apps register their own
 * skills (Excel / PPT) against the same agent loop.
 */
export function createDocsSkill(
  getEditor: () => Editor,
  getNumIds: () => NumIds,
  getTrack?: () => AiTrack | undefined,
): AgentSkill {
  return {
    id: 'docx',
    systemPrompt: AGENT_SYSTEM_PROMPT,
    tools: AGENT_TOOLS,
    buildContext: () => {
      const editor = getEditor()
      markDocSeen(editor) // the context the model receives is the freshness baseline for index-addressed writes
      return buildDocContext(editor)
    },
    // Each edit the agent makes shows as its labelled cursor typing it in (agent-presence.ts)
    executeTool: (call, signal) => {
      const editor = getEditor()
      const before = editor.state.doc
      const shown = <T,>(exec: T): T => {
        if ((exec as { mutated?: boolean } | null)?.mutated) showAgentEdit(editor, before)
        return exec
      }
      const exec = executeTool(editor, call, getNumIds(), getTrack?.(), signal)
      return exec instanceof Promise ? exec.then(shown) : shown(exec)
    },
    anchor: () => describeDocAnchor(getEditor()),
  }
}

/** `"Say it once, then prove it."` — the selection, else the cursor's block; null in an empty spot. */
export function describeDocAnchor(editor: Editor): string | null {
  const { from, to, $from } = editor.state.selection
  const raw = from < to ? editor.state.doc.textBetween(from, to, ' ') : $from.parent.textContent
  const text = raw.replace(/\s+/g, ' ').trim()
  if (!text) return null
  return `"${text.length > 48 ? `${text.slice(0, 48)}…` : text}"`
}
