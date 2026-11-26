/**
 * Needs-you prompts from the agent browser (computer.credential,
 * computer.human, computer.block; runtime/tools/computer-toolset/needs-you.ts).
 * The runtime asks through PermissionNext with `metadata.summary`,
 * `metadata.options` (the buttons) and `metadata.secret` (a masked input).
 * This reads those fields for every TUI view that shows a runtime permission
 * ask (bot chat dialog, /decisions) so each shows the same buttons.
 *
 * Reply mapping (PermissionNext.reply): "once" = first button, "task" = the
 * second button of credential/block (arrives as ReplyData.alt), "reject" = the
 * deny/stop button. A secret's value goes in the reply's `answer`.
 */

export const NEEDS_YOU_PERMISSIONS = ['computer.credential', 'computer.human', 'computer.block'] as const

export type NeedsYouReply = 'once' | 'task' | 'reject'

export type NeedsYouOption = { reply: NeedsYouReply; label: string }

export type NeedsYouView = {
  summary: string
  options: NeedsYouOption[]
  /** Ask for a masked value before sending a non-reject reply. */
  secret: boolean
  /** Short label for the secret field. */
  secretLabel: string
}

export function isNeedsYouPermission(permission: string | undefined): boolean {
  return !!permission && (NEEDS_YOU_PERMISSIONS as readonly string[]).includes(permission)
}

const REPLIES: readonly string[] = ['once', 'task', 'reject']

/** Defaults match what the runtime sends, for an older runtime that omits `options`. */
function defaultOptions(permission: string): NeedsYouOption[] {
  if (permission === 'computer.credential') {
    return [
      { reply: 'once', label: 'Approve' },
      { reply: 'task', label: 'Allow for this run' },
      { reply: 'reject', label: 'Deny' },
    ]
  }
  if (permission === 'computer.block') {
    return [
      { reply: 'once', label: 'Continue' },
      { reply: 'task', label: 'Re-run on my Mac' },
      { reply: 'reject', label: 'Stop' },
    ]
  }
  return [
    { reply: 'once', label: 'Continue' },
    { reply: 'reject', label: 'Stop' },
  ]
}

export function needsYouView(permission: string, metadata: unknown, patterns: string[] = []): NeedsYouView {
  const m = (metadata && typeof metadata === 'object' ? metadata : {}) as Record<string, unknown>
  const fromMeta = Array.isArray(m.options)
    ? (m.options as unknown[])
        .map((o): NeedsYouOption | undefined => {
          const r = o as { reply?: unknown; label?: unknown }
          return typeof r?.reply === 'string' && REPLIES.includes(r.reply) && typeof r.label === 'string' && r.label
            ? { reply: r.reply as NeedsYouReply, label: r.label }
            : undefined
        })
        .filter((o): o is NeedsYouOption => !!o)
    : []
  const options = fromMeta.length ? fromMeta : defaultOptions(permission)
  const summary =
    typeof m.summary === 'string' && m.summary
      ? m.summary
      : `${permission.replace('computer.', 'Agent browser: ')} ${patterns[0] ?? ''}`.trim()
  return {
    summary,
    options,
    secret: m.secret === true,
    secretLabel: typeof m.label === 'string' && m.label ? m.label : 'Value',
  }
}

/** Masked display of a typed value: bullets only, never the text. */
export function maskSecret(value: string): string {
  return '•'.repeat(value.length)
}
