import { expect, test } from "bun:test"
import edit from "../../src/cli/ui/ink-app/commands/edit/index"
import { call, NO_MESSAGE_NOTICE } from "../../src/cli/ui/ink-app/commands/edit/edit"

test("/edit is named with alias", () => {
  expect(edit.name).toBe("edit")
  expect(edit.aliases).toContain("edit-last")
})
test("/edit with no user message gives a one-line notice", async () => {
  expect(await call("", {} as any)).toEqual({ type: "text", value: NO_MESSAGE_NOTICE })
  expect(await call("", { restoreLastUserMessage: () => false } as any)).toEqual({ type: "text", value: NO_MESSAGE_NOTICE })
})
test("/edit restores when a user message exists", async () => {
  expect(await call("", { restoreLastUserMessage: () => true } as any)).toEqual({ type: "skip" })
})
