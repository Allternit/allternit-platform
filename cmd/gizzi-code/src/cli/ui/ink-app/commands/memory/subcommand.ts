/**
 * /memory owns memory-search and remember (overhaul plan D4): `/memory search
 * <words>` and `/memory add <note>` run them; the old names stay as hidden
 * aliases for one release. `/memory sleep` with no on/off opens the Sleep
 * view when this build registers the `sleep` command (PR #3235).
 */
export type MemoryRoute =
  | { kind: 'search'; rest: string }
  | { kind: 'add'; rest: string }
  | { kind: 'sleep-view' }

export function memoryRoute(args: string | undefined): MemoryRoute | undefined {
  const trimmed = (args ?? '').trim()
  const [word = '', ...restWords] = trimmed.split(/\s+/)
  const rest = restWords.join(' ')
  switch (word.toLowerCase()) {
    case 'search':
    case 'find':
      return { kind: 'search', rest }
    case 'add':
    case 'remember':
    case 'save':
      return { kind: 'add', rest }
    case 'sleep':
      return rest.trim() === '' ? { kind: 'sleep-view' } : undefined
    default:
      return undefined
  }
}

/** The name of the Sleep view command (registered by PR #3235). */
export const SLEEP_COMMAND_NAME = 'sleep'
