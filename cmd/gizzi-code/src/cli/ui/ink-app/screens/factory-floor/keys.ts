/**
 * Pure key dispatcher for the factory floor. FactoryFloorScreen's useInput
 * funnels keys through here; tests drive the same function.
 */
export interface FactoryFloorKeyHandlers {
  moveUp(): void
  moveDown(): void
  /** Tab — switch focus between the bots list and the needs-you list. */
  switchFocus(): void
  /** Enter — open the selected bot (attach / session / vendor thread). */
  openSelected(): void
  /** a — approve the selected needs-you node. */
  approveSelected(): void
  /** w — hand the screen to the engine's live wall. */
  openWall(): void
  /** r — reload from the engine. */
  refresh(): void
  /** q / Esc — back to the REPL. */
  exit(): void
}

export interface FactoryFloorKey {
  escape?: boolean
  return?: boolean
  tab?: boolean
  upArrow?: boolean
  downArrow?: boolean
  ctrl?: boolean
  meta?: boolean
}

export function handleFactoryFloorKey(input: string, key: FactoryFloorKey, h: FactoryFloorKeyHandlers): boolean {
  if (key.escape) {
    h.exit()
    return true
  }
  if (key.ctrl || key.meta) return false
  if (key.upArrow || input === "k") {
    h.moveUp()
    return true
  }
  if (key.downArrow || input === "j") {
    h.moveDown()
    return true
  }
  if (key.tab) {
    h.switchFocus()
    return true
  }
  if (key.return) {
    h.openSelected()
    return true
  }
  switch (input) {
    case "a":
      h.approveSelected()
      return true
    case "w":
      h.openWall()
      return true
    case "r":
      h.refresh()
      return true
    case "q":
      h.exit()
      return true
    default:
      return false
  }
}
