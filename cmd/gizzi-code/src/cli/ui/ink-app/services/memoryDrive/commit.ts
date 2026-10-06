/**
 * Fold memory edits the TUI agent (or extract-memories / auto-dream forks)
 * made with file tools in the Memory Drive checkout into one validated
 * commit and sync it. Everything goes through the common drive writer
 * (DriveCheckout.commitWorkingTree): format/secret checks over the whole
 * tree, invalid edits moved aside visibly, push with bounded retry, offline
 * pending state. Best-effort and quiet: problems surface in /memory and in
 * the next session's memory context, never as a thrown error mid-turn.
 */
import { getSessionId } from '../../bootstrap/state.js'
import { isAutoMemoryEnabled, isMemoryDriveActive } from '../../memdir/paths.js'
import { logForDebugging } from '../../utils/debug.js'
import { MemoryDrive } from '../../../../../runtime/memory/drive/drive.js'

export async function commitMemoryDriveEdits(message: string): Promise<void> {
  if (!isAutoMemoryEnabled() || !isMemoryDriveActive()) return
  try {
    const result = await MemoryDrive.commitWorkingTree({
      sessionId: String(getSessionId() ?? ''),
      message,
    })
    if (result.error) logForDebugging(`[memoryDrive] ${result.error}`)
  } catch (error) {
    logForDebugging(`[memoryDrive] commit failed: ${error instanceof Error ? error.message : String(error)}`)
  }
}
