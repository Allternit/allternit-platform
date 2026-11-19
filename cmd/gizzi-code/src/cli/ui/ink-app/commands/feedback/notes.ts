import { FeedbackNotes } from '@/runtime/primitives/feedback-notes'

function when(ts: number): string {
  return new Date(ts).toISOString().replace('T', ' ').slice(0, 16)
}

/** `/feedback notes` lists saved notes, newest first. `/feedback notes delete <n|id>` removes one. */
export async function runNotesCommand(args: string): Promise<string> {
  const notes = (await FeedbackNotes.list()).slice().sort((a, b) => b.created_at - a.created_at)
  const del = args.trim().match(/^(?:delete|remove|rm)\s+(\S+)$/i)
  if (del) {
    const key = del[1]!
    const idx = /^\d+$/.test(key) ? Number(key) - 1 : -1
    const target = idx >= 0 ? notes[idx] : notes.find(n => n.id === key)
    if (!target) return `No note matches "${key}". Run /feedback notes to see the list.`
    await FeedbackNotes.remove(target.id)
    return `Deleted note: ${target.summary}`
  }
  if (notes.length === 0) return 'No feedback notes saved on this device.'
  const lines = notes.map(
    (n, i) => `${i + 1}. [${n.kind}] ${n.summary}  (${when(n.created_at)}, id ${n.id})`,
  )
  return `Feedback notes saved on this device (newest first). Nothing is sent until you send it.\n${lines.join('\n')}\n\nDelete one with /feedback notes delete <number>.`
}
