import { BusEvent } from "@/shared/bus/bus-event"
import z from "zod/v4"
import { Config } from "@/runtime/context/config/config"
import { Instance } from "@/runtime/context/project/instance"
import { Identifier } from "@/shared/id/id"
import PROMPT_INITIALIZE from "./prompts/initialize.txt"
import PROMPT_REVIEW from "./prompts/review.txt"
import { MCP } from "@/runtime/tools/mcp"
import { Skill } from "@/runtime/skills/skill"

export namespace Command {
  export const Event = {
    Executed: BusEvent.define(
      "command.executed",
      z.object({
        name: z.string(),
        sessionID: Identifier.schema("session"),
        arguments: z.string(),
        messageID: Identifier.schema("message"),
      }),
    ),
  }

  export const Info = z
    .object({
      name: z.string(),
      description: z.string().optional(),
      agent: z.string().optional(),
      model: z.string().optional(),
      source: z.enum(["command", "mcp", "skill"]).optional(),
      // workaround for zod not supporting async functions natively so we use getters
      // https://zod.dev/v4/changelog?id=zfunction
      template: z.promise(z.string()).or(z.string()),
      subtask: z.boolean().optional(),
      hints: z.array(z.string()),
    })
    

  // for some reason zod is inferring `string` for z.promise(z.string()).or(z.string()) so we have to manually override it
  export type Info = Omit<z.infer<typeof Info>, "template"> & { template: Promise<string> | string }

  export function hints(template: string): string[] {
    const result: string[] = []
    const numbered = template.match(/\$\d+/g)
    if (numbered) {
      for (const match of [...new Set(numbered)].sort()) result.push(match)
    }
    if (template.includes("$ARGUMENTS")) result.push("$ARGUMENTS")
    return result
  }

  export const Default = {
    INIT: "init",
    REVIEW: "review",
  }

  export const list = Instance.state(async () => {
    const cfg = await Config.get()
    const result: Record<string, Info> = {
      [Default.INIT]: {
        name: Default.INIT,
        description: "Initialize a new project",
        template: PROMPT_INITIALIZE,
        hints: [],
      },
      [Default.REVIEW]: {
        name: Default.REVIEW,
        description: "Review recent changes (presets: security, perf, tests, diff)",
        template: PROMPT_REVIEW,
        hints: [],
      },
    }

    return result
  })

  export async function get(name: string) {
    return (await list())[name]
  }

  export async function all() {
    return Object.values(await list())
  }
}
