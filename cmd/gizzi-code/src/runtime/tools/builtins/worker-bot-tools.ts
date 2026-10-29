/**
 * Worker bots and vendor connect, from a bot's chat.
 *
 * - bot_create: make a worker bot for this bot's owner that owns kinds of
 *   tasks (`owns`); teammates see `owns` on the roster and hand that work to it.
 * - vendor_connect: show the user a card that connects a vendor CLI (any entry
 *   in the CLI catalog). The tool never starts a sign-in and never carries a
 *   link or a code; the app's card runs the install/connect endpoints and the
 *   only codes the user sees come from the vendor CLI's own output.
 *
 * Only in an app bot's own chat (gated in SessionPrompt.resolveTools with
 * isAppBotSession, like suggest_task). The owner comes from the calling
 * session, server side. See cmd/allternit-api/src/worker_bot_tools.rs.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { AppRoutingError, createAppBot, vendorConnectCard } from "@/runtime/bots/app-bot-routing"

const errorText = (error: unknown) => (error instanceof AppRoutingError ? error.message : (error as Error).message)

const BOT_CREATE_DESCRIPTION = `Create a worker bot for your user that owns kinds of tasks.

Use it when your user asks for a bot to take over a kind of work (for example
"make a bot that handles invoices"). Other bots see what it owns and hand that
work to it with message_agent.

- name: the new bot's name.
- purpose: optional, one or two sentences on what it is for.
- owns: the kinds of tasks it owns, short phrases (e.g. ["invoices", "expense reports"]).
- brain: optional {provider, model?}; provider is a vendor id from the CLI
  catalog (e.g. "claude-code", "codex-cli", "opencode"). If that vendor isn't
  connected yet, offer vendor_connect.
- computer: optional "cloud", "local", or a computer id.`

export const BotCreateTool = Tool.define("bot_create", {
  description: BOT_CREATE_DESCRIPTION,
  parameters: z.object({
    name: z.string().describe("The new bot's name"),
    purpose: z.string().optional().describe("What the bot is for"),
    owns: z.array(z.string()).describe("Kinds of tasks the bot owns"),
    brain: z
      .object({ provider: z.string().describe("Vendor id from the CLI catalog"), model: z.string().optional() })
      .optional(),
    computer: z.string().optional().describe('"cloud", "local", or a computer id'),
  }),
  async execute(params, ctx) {
    try {
      const r = await createAppBot(ctx.sessionID, params)
      return {
        title: `Created ${r.bot.name}`,
        output: `Created ${r.bot.name} (id ${r.bot.id}). It owns: ${r.bot.owns.join(", ") || "(nothing yet)"}. Hand that work to it with message_agent.`,
        metadata: { botId: r.bot.id as string | undefined, owns: r.bot.owns },
      }
    } catch (error) {
      return { title: "Bot not created", output: errorText(error), metadata: { botId: undefined as string | undefined, owns: [] as string[] } }
    }
  },
})

const VENDOR_CONNECT_DESCRIPTION = `Show your user a card that connects a vendor (an AI CLI such as Claude Code, Codex, Grok, Gemini or OpenCode).

Use it when the user asks to connect or sign in to a vendor, or when a brain
you need is not connected. The card installs the CLI and runs its sign-in; the
user follows the steps on the card. Never paste sign-in links or codes
yourself, and never ask the user for a code or a key in chat.

- provider: the vendor id from the CLI catalog (e.g. "codex-cli", "grok", "opencode").
- computer: optional "cloud", "local", or a computer id to connect it on.`

export const VendorConnectTool = Tool.define("vendor_connect", {
  description: VENDOR_CONNECT_DESCRIPTION,
  parameters: z.object({
    provider: z.string().describe("Vendor id from the CLI catalog"),
    computer: z.string().optional().describe('"cloud", "local", or a computer id'),
  }),
  async execute(params, ctx) {
    try {
      const card = await vendorConnectCard(ctx.sessionID, params)
      return {
        title: `Connect ${card.label}`,
        output: JSON.stringify(card),
        metadata: { card: card as typeof card | undefined },
      }
    } catch (error) {
      return { title: "Can't connect", output: errorText(error), metadata: { card: undefined } }
    }
  },
})

export const WORKER_BOT_TOOL_IDS = [BotCreateTool.id, VendorConnectTool.id] as const
