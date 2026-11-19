/**
 * user_location (prompt-gap contract stub; built in G7 (PR4)). The person's location by a client round trip; approximate by default.
 * Shapes in runtime/primitives/gap-types.ts.
 *
 * Gated: the registry, the CLI bridge and the tool families list this tool
 * only while `available` is true. The lane that builds it sets `available`
 * here (one line) when execute is real; nothing else needs to change.
 */
import { Tool } from "@/runtime/tools/builtins/tool"
import { USER_LOCATION_TOOL_ID, UserLocationInput, type UserLocationMetadata } from "@/runtime/primitives/gap-types"
import DESCRIPTION from "@/runtime/tools/builtins/user-location.txt"

export const available: boolean = false

export const UserLocationTool = Tool.define<typeof UserLocationInput, UserLocationMetadata>(USER_LOCATION_TOOL_ID, {
  description: DESCRIPTION,
  parameters: UserLocationInput,
  async execute(_params) {
    return {
      title: "user_location is not available yet",
      output: "This tool is not available in this version.",
      metadata: { ok: false },
    }
  },
})
