import { describe, expect, test } from "bun:test"
import { SessionPrompt } from "@/runtime/session/prompt"

describe("session.prompt mobile tool options (metadata.tools)", () => {
  describe("parseMobileToolOptions", () => {
    test("returns undefined when metadata is absent", () => {
      expect(SessionPrompt.parseMobileToolOptions(undefined)).toBeUndefined()
    })

    test("returns undefined when metadata has no tools key", () => {
      expect(SessionPrompt.parseMobileToolOptions({ unrelated: true })).toBeUndefined()
    })

    test("parses the bridge wire shape", () => {
      expect(
        SessionPrompt.parseMobileToolOptions({
          tools: { webSearch: false, research: false, toolAccess: "always" },
        }),
      ).toEqual({ webSearch: false, research: false, toolAccess: "always" })
    })

    test("tolerates unknown extra keys in tools", () => {
      expect(
        SessionPrompt.parseMobileToolOptions({ tools: { webSearch: true, futureOption: 1 } }),
      ).toEqual({ webSearch: true })
    })

    test("returns undefined for malformed tools instead of throwing", () => {
      expect(SessionPrompt.parseMobileToolOptions({ tools: "nope" })).toBeUndefined()
      expect(SessionPrompt.parseMobileToolOptions({ tools: { toolAccess: "sometimes" } })).toBeUndefined()
    })
  })

  describe("applyMobileToolGating", () => {
    const toolset = () =>
      ({ websearch: {}, task: {}, bash: {}, read: {} }) as unknown as Parameters<
        typeof SessionPrompt.applyMobileToolGating
      >[0]

    test("webSearch:false removes only websearch", () => {
      const tools = toolset()
      SessionPrompt.applyMobileToolGating(tools, { webSearch: false })
      expect(Object.keys(tools).sort()).toEqual(["bash", "read", "task"])
    })

    test("research:false removes websearch and task", () => {
      const tools = toolset()
      SessionPrompt.applyMobileToolGating(tools, { research: false })
      expect(Object.keys(tools).sort()).toEqual(["bash", "read"])
    })

    test("webSearch:true and research:true leave the tool set unchanged", () => {
      const tools = toolset()
      SessionPrompt.applyMobileToolGating(tools, { webSearch: true, research: true })
      expect(Object.keys(tools).sort()).toEqual(["bash", "read", "task", "websearch"])
    })

    test("toolAccess alone does not filter tools", () => {
      const tools = toolset()
      SessionPrompt.applyMobileToolGating(tools, { toolAccess: "always" })
      expect(Object.keys(tools).sort()).toEqual(["bash", "read", "task", "websearch"])
    })

    test("missing tools are tolerated (idempotent deletes)", () => {
      const tools = { bash: {} } as unknown as Parameters<typeof SessionPrompt.applyMobileToolGating>[0]
      SessionPrompt.applyMobileToolGating(tools, { webSearch: false, research: false })
      expect(Object.keys(tools)).toEqual(["bash"])
    })
  })
})

describe("disabledConnectorFor", () => {
  const options = SessionPrompt.parseMobileToolOptions({ tools: { disabledConnectors: ["GitHub", "notion"] } })

  test("parses disabledConnectors from metadata.tools", () => {
    expect(options?.disabledConnectors).toEqual(["GitHub", "notion"])
  })

  test("blocks actions of a connector turned off for the session", () => {
    expect(SessionPrompt.disabledConnectorFor(options, { actionId: "github.get_current_user" })).toBe("GitHub")
    expect(SessionPrompt.disabledConnectorFor(options, { actionId: "notion.search", input: {} })).toBe("notion")
  })

  test("leaves other connectors and non-action calls alone", () => {
    expect(SessionPrompt.disabledConnectorFor(options, { actionId: "slack.post_message" })).toBeUndefined()
    expect(SessionPrompt.disabledConnectorFor(options, { query: "github issues" })).toBeUndefined()
    expect(SessionPrompt.disabledConnectorFor(undefined, { actionId: "github.x" })).toBeUndefined()
  })
})
