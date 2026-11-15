import { cmd } from "@/cli/commands/cmd"
import { UI } from "@/cli/ui"
import open from "open"

const LABS_URL = "https://labs.allternit.com"

export const LabsCommand = cmd({
  command: "labs [action]",
  describe: "Open A://Labs courses (labs.allternit.com)",
  builder: (yargs) =>
    yargs.positional("action", {
      type: "string",
      choices: ["courses", "lessons"],
      describe: "Kept for compatibility; both open the same page",
    }),

  handler: async () => {
    UI.println(`A://Labs courses: ${LABS_URL}`)
    if (process.stdout.isTTY) {
      try {
        await open(LABS_URL)
      } catch {
        // The URL is already printed; opening the browser is best effort.
      }
    }
  },
})
