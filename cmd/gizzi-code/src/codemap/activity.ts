/**
 * Live code-map activity: which project file the agent just read or edited.
 * The app's code map lights files blue (read) and coral (edit) from these.
 * Paths are relative to the session's project directory; files outside it
 * are not published.
 */
import path from "path"
import z from "zod/v4"
import { BusEvent } from "@/shared/bus/bus-event"
import { Bus } from "@/shared/bus"
import { Instance } from "@/runtime/context/project/instance"
import { Log } from "@/shared/util/log"

export namespace CodeMapActivity {
  const log = Log.create({ service: "codemap.activity" })

  export const Kind = z.enum(["read", "edit"])
  export type Kind = z.infer<typeof Kind>

  export const Event = {
    Activity: BusEvent.define(
      "codemap.activity",
      z.object({
        sessionID: z.string(),
        /** Project-relative, forward slashes. */
        path: z.string(),
        kind: Kind,
      }),
    ),
  }

  /** `file` relative to `root`, or undefined when it is outside it. */
  export function relative(root: string, file: string): string | undefined {
    const abs = path.isAbsolute(file) ? file : path.join(root, file)
    const rel = path.relative(path.resolve(root), path.resolve(abs))
    if (!rel || rel.startsWith("..") || path.isAbsolute(rel)) return undefined
    return rel.split(path.sep).join("/")
  }

  /** Publish a read/edit for the map. Never throws, never awaited by tools. */
  export function publish(sessionID: string | undefined, file: string, kind: Kind) {
    if (!sessionID) return
    try {
      const rel = relative(Instance.directory, file)
      if (!rel) return
      void Bus.publish(Event.Activity, { sessionID, path: rel, kind }).catch((error) =>
        log.warn("publish failed", { error: String(error) }),
      )
    } catch (error) {
      log.warn("publish failed", { error: String(error) })
    }
  }
}
