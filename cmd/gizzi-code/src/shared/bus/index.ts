import z from "zod/v4"
import { Log } from "@/shared/util/log"
import { Instance } from "@/runtime/context/project/instance"
import { BusEvent } from "@/shared/bus/bus-event"
import { GlobalBus } from "@/shared/bus/global"
import { SecretRedactor } from "@/runtime/session/environment/redactor"

export namespace Bus {
  const log = Log.create({ service: "bus" })
  type Subscription = (event: any) => void

  export const InstanceDisposed = BusEvent.define(
    "server.instance.disposed",
    z.object({
      directory: z.string(),
    }),
  )

  const HISTORY_LIMIT = 500

  const state = Instance.state(
    () => {
      const subscriptions = new Map<any, Subscription[]>()
      const history: { seq: number; event: any }[] = []

      return {
        subscriptions,
        history,
        seq: 0,
      }
    },
    async (entry) => {
      const wildcard = entry.subscriptions.get("*")
      if (!wildcard) return
      const event = {
        type: InstanceDisposed.type,
        properties: {
          directory: Instance.directory,
        },
      }
      for (const sub of [...wildcard]) {
        sub(event)
      }
    },
  )

  export async function publish<Definition extends BusEvent.Definition>(
    def: Definition,
    properties: z.output<Definition["properties"]>,
  ) {
    const payload = {
      type: def.type,
      // Cloud environment secrets never leave the process on the bus.
      properties: SecretRedactor.deep(properties),
    }
    log.info("publishing", {
      type: def.type,
    })

    // Record in the replay buffer *before* notifying subscribers, so a
    // subscriber reading currentSeq() synchronously (e.g. the SSE route
    // tagging its outgoing frame) observes this event's own seq number.
    const s = state()
    const seq = ++s.seq
    s.history.push({ seq, event: payload })
    if (s.history.length > HISTORY_LIMIT) {
      s.history.splice(0, s.history.length - HISTORY_LIMIT)
    }

    const pending = []
    for (const key of [def.type, "*"]) {
      const match = state().subscriptions.get(key)
      for (const sub of match ?? []) {
        pending.push(sub(payload))
      }
    }
    GlobalBus.emit("event", {
      directory: Instance.directory,
      payload,
    })
    return Promise.all(pending)
  }

  /** Events published after `seq`, oldest first — for SSE clients resuming after a drop. */
  export function historySince(seq: number): { seq: number; event: any }[] {
    return state().history.filter((entry) => entry.seq > seq)
  }

  /** The seq number of the most recently published event (0 if none yet). */
  export function currentSeq(): number {
    return state().seq
  }

  export function subscribe<Definition extends BusEvent.Definition>(
    def: Definition,
    callback: (event: { type: Definition["type"]; properties: z.infer<Definition["properties"]> }) => void,
  ) {
    return raw(def.type, callback)
  }

  export function once<Definition extends BusEvent.Definition>(
    def: Definition,
    callback: (event: {
      type: Definition["type"]
      properties: z.infer<Definition["properties"]>
    }) => "done" | undefined,
  ) {
    const unsub = subscribe(def, (event) => {
      if (callback(event)) unsub()
    })
  }

  export function subscribeAll(callback: (event: any) => void) {
    return raw("*", callback)
  }

  function raw(type: string, callback: (event: any) => void) {
    log.info("subscribing", { type })
    const subscriptions = state().subscriptions
    let match = subscriptions.get(type) ?? []
    match.push(callback)
    subscriptions.set(type, match)

    return () => {
      log.info("unsubscribing", { type })
      const match = subscriptions.get(type)
      if (!match) return
      const index = match.indexOf(callback)
      if (index === -1) return
      match.splice(index, 1)
    }
  }
}
