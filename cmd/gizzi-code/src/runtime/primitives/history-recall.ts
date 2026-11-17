/**
 * History recall (Primitives Wave B, lane L4): reads this session's
 * compacted messages and the sessions in its handoff lineage. Read-only;
 * never reaches another session tree. At most HISTORY_RECALL_READ_MAX_CHARS
 * per read, and every result says the text is data, not instructions.
 *
 * Contract stub (B1): throws until L4 fills it in.
 */
import { notImplemented, type HistoryRecallInput, type HistoryRecallResult } from "@/runtime/primitives/wave-b-types"

export namespace HistoryRecall {
  export function run(_input: { sessionID: string } & HistoryRecallInput): Promise<HistoryRecallResult> {
    return notImplemented("history_recall")
  }
}
