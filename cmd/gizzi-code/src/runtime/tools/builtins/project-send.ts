/**
 * Work across projects from a bot's main chat.
 *
 * - project_list: the user's open projects.
 * - project_send: send a written brief into one project. The project's own
 *   coordinator takes it from there, with its own team, instructions and
 *   permissions. The report comes back to this chat when its threads finish.
 *
 * Every send asks the person. `project_send` is a PermissionNext.ALWAYS_ASK
 * class: every mode asks, and no earlier answer or task grant covers the next
 * send. The project title on the card comes from the server, not the model.
 *
 * Only in a bot's main chat (gated in SessionPrompt.resolveTools with
 * isAppBotMainChat; the server checks again). The owner comes from the calling
 * session. See cmd/allternit-api/src/project_send_tools.rs.
 */
import z from "zod/v4"
import { Tool } from "@/runtime/tools/builtins/tool"
import { AppRoutingError, listAppProjects, sendToAppProject } from "@/runtime/bots/app-bot-routing"

const errorText = (error: unknown) => (error instanceof AppRoutingError ? error.message : (error as Error).message)

export const ProjectListTool = Tool.define("project_list", {
  description: `List your user's open projects, with the id project_send needs.

Use it when your user names a project or asks you to work in one.`,
  parameters: z.object({}),
  async execute(_params, ctx) {
    try {
      const projects = await listAppProjects(ctx.sessionID)
      return {
        title: `${projects.length} project${projects.length === 1 ? "" : "s"}`,
        output: projects.length
          ? projects.map((p) => `- ${p.title} (${p.mode}, id ${p.id})${p.description ? `: ${p.description}` : ""}`).join("\n")
          : "This user has no open projects yet.",
        metadata: { count: projects.length },
      }
    } catch (error) {
      return { title: "Projects not listed", output: errorText(error), metadata: { count: 0 } }
    }
  },
})

export const ProjectSendTool = Tool.define("project_send", {
  description: `Send work into one of your user's projects.

Use it when your user asks for something that belongs in another project. The
project plans the work and runs it in its own threads, with its own files,
instructions and permissions. Your user approves every send on a card that
shows the project and your full message.

- project_id: from project_list.
- message: a complete brief. The project cannot see this chat, so include the
  links, context and what "done" means. State that nothing goes live without
  your user's review when that applies.

A report comes back to this chat when the threads finish. Do not wait for it
and do not send the same work twice.`,
  parameters: z.object({
    project_id: z.string().describe("The project id, from project_list"),
    message: z.string().describe("The complete brief for the project"),
  }),
  async execute(params, ctx) {
    const message = params.message.trim()
    const failed = (output: string) => ({ title: "Not sent", output, metadata: { sent: false, threads: [] as string[] } })
    if (!message) return failed("The message is empty.")
    let title: string
    try {
      const project = (await listAppProjects(ctx.sessionID)).find((p) => p.id === params.project_id.trim())
      if (!project) return failed("No open project has that id. Call project_list.")
      title = project.title
    } catch (error) {
      return failed(errorText(error))
    }
    await ctx.ask({
      permission: "project_send",
      patterns: [params.project_id.trim()],
      always: [],
      metadata: {
        summary: `Send to ${title}`,
        projectSend: { projectId: params.project_id.trim(), projectTitle: title, message },
      },
    })
    try {
      const r = await sendToAppProject(ctx.sessionID, params.project_id.trim(), message)
      const count = r.threads.length
      return {
        title: `Sent to ${r.project.title}`,
        output: `Sent to ${r.project.title}. ${count ? `It started ${count} thread${count === 1 ? "" : "s"}; a report comes back here when they finish.` : "It started no threads."}${r.message?.text ? `\nThe project replied: ${r.message.text}` : ""}`,
        metadata: { sent: true, threads: r.threads },
      }
    } catch (error) {
      return failed(errorText(error))
    }
  },
})

export const PROJECT_SEND_TOOL_IDS = [ProjectListTool.id, ProjectSendTool.id] as const
