/**
 * Pure key dispatcher for the /bots pane (Phase B5). BotsPaneScreen's
 * useInput funnels list-focus keys through here; unit tests drive the same
 * function to assert each keybinding reaches the right callback.
 */
export interface BotsPaneKeyHandlers {
  moveUp(): void
  moveDown(): void
  /** Enter — open the selected bot's canonical chat. */
  openSelected(): void
  /** n — prompt for name/title and create a bot. */
  createNew(): void
  /** d — delete the selected bot (inline y/n confirm). */
  deleteSelected(): void
  /** r — refresh roster rows. */
  refresh(): void
  /** f — the Allternit Factory floor. */
  openFactory?(): void
  /** q / Esc — back to the REPL. */
  exit(): void
}

export interface BotsPaneKey {
  escape?: boolean
  return?: boolean
  upArrow?: boolean
  downArrow?: boolean
  ctrl?: boolean
  meta?: boolean
}

/**
 * Dispatch one key event to the matching handler. Returns true when the key
 * was claimed. Modifier chords (ctrl/meta, except plain escape) are left
 * unclaimed so global bindings keep working.
 */
export function handleBotsPaneKey(
  input: string,
  key: BotsPaneKey,
  handlers: BotsPaneKeyHandlers,
): boolean {
  if (key.escape) {
    handlers.exit()
    return true
  }
  if (key.ctrl || key.meta) return false
  if (key.upArrow || input === 'k') {
    handlers.moveUp()
    return true
  }
  if (key.downArrow || input === 'j') {
    handlers.moveDown()
    return true
  }
  if (key.return) {
    handlers.openSelected()
    return true
  }
  switch (input) {
    case 'n':
      handlers.createNew()
      return true
    case 'd':
      handlers.deleteSelected()
      return true
    case 'r':
      handlers.refresh()
      return true
    case 'f':
      if (!handlers.openFactory) return false
      handlers.openFactory()
      return true
    case 'q':
      handlers.exit()
      return true
    default:
      return false
  }
}
