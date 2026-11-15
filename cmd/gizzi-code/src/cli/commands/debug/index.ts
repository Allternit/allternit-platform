import { cmd } from "@/cli/commands/cmd"
import { DebugCommand as BaseDebugCommand } from "./debug"
import { ModelCommand } from "./model"

// The base command owns the sibling subcommands; add `debug model` on top.
const baseBuilder = BaseDebugCommand.builder as (yargs: any) => any

export const DebugCommand = cmd({
  command: BaseDebugCommand.command as string,
  describe: BaseDebugCommand.describe as string,
  builder: (yargs: any) => baseBuilder(yargs).command(ModelCommand),
  handler: async () => {},
})
