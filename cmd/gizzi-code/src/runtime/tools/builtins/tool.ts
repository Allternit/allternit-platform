import z from "zod/v4"
import type { MessageV2 } from "@/runtime/session/message-v2"
import type { Agent } from "@/runtime/loop/agent"
import type { PermissionNext } from "@/runtime/tools/guard/permission/next"
import { Truncate } from "@/runtime/tools/builtins/truncation"
import { ToolValidationRetry } from "@/runtime/tools/validation-retry"
import { LabelField, ToolCallLabel } from "@/runtime/primitives/gap-types"

export namespace Tool {
  interface Metadata {
    [key: string]: any
  }

  export interface InitContext {
    agent?: Agent.Info
    /** The model the tool set is being built for (adapters pick their wire shape from it). */
    model?: { providerID: string; modelID: string; npm?: string }
    /** The session the tool set is being built for, when there is one. */
    sessionID?: string
  }

  export type Context<M extends Metadata = Metadata> = {
    sessionID: string
    messageID: string
    agent: string
    abort: AbortSignal
    callID?: string
    extra?: { [key: string]: any }
    messages: MessageV2.WithParts[]
    metadata(input: { title?: string; metadata?: M }): void
    /**
     * Ask the person (per the permission rules). Resolves when allowed; an
     * always-ask class (PermissionNext.ALWAYS_ASK) resolves with the reply data
     * the confirming surface attached — a context without a person resolves
     * with nothing.
     */
    ask(input: Omit<PermissionNext.Request, "id" | "sessionID" | "tool">): Promise<PermissionNext.ReplyData | void>
  }
  export interface Info<Parameters extends z.ZodType = z.ZodType, M extends Metadata = Metadata> {
    id: string
    init: (ctx?: InitContext) => Promise<{
      description: string
      parameters: Parameters
      execute(
        args: z.infer<Parameters>,
        ctx: Context,
      ): Promise<{
        title: string
        metadata: M
        output: string
        attachments?: Omit<MessageV2.FilePart, "id" | "sessionID" | "messageID">[]
      }>
      formatValidationError?(error: z.ZodError): string
    }>
  }

  export type InferParameters<T extends Info> = T extends Info<infer P> ? z.infer<P> : never
  export type InferMetadata<T extends Info> = T extends Info<any, infer M> ? M : never

  export function define<Parameters extends z.ZodType, Result extends Metadata>(
    id: string,
    init: Info<Parameters, Result>["init"] | Awaited<ReturnType<Info<Parameters, Result>["init"]>>,
  ): Info<Parameters, Result> {
    return {
      id,
      init: async (initCtx) => {
        const toolInfo = init instanceof Function ? await init(initCtx) : init
        const execute = toolInfo.execute
        const labelled = takesLabel(toolInfo.parameters)
        toolInfo.execute = async (rawArgs, ctx) => {
          // PR8a: `label` is for the person watching, not the tool. Take it off
          // before validation and execute; a bad label is dropped, never fatal.
          const { args, label } = labelled ? splitLabel(rawArgs) : { args: rawArgs, label: undefined }
          if (label) {
            const report = ctx.metadata.bind(ctx)
            ctx = { ...ctx, metadata: (input) => report({ ...input, metadata: { ...input.metadata, label } as any }) }
            ctx.metadata({})
          }
          try {
            toolInfo.parameters.parse(args)
          } catch (error) {
            if (error instanceof z.ZodError) {
              const count = ToolValidationRetry.recordFailure(ctx.sessionID, id, ToolValidationRetry.summarize(error))
              const capped = count >= ToolValidationRetry.MAX_CONSECUTIVE_FAILURES
              const base = toolInfo.formatValidationError
                ? toolInfo.formatValidationError(error)
                : `The ${id} tool was called with invalid arguments: ${ToolValidationRetry.summarize(error)}.\nPlease rewrite the input so it satisfies the expected schema.`
              const message = capped
                ? `${base}\nThis is attempt ${count} of ${ToolValidationRetry.MAX_CONSECUTIVE_FAILURES} with invalid arguments for ${id} — stop retrying this exact approach and try something else.`
                : `${base} (attempt ${count}/${ToolValidationRetry.MAX_CONSECUTIVE_FAILURES})`
              const validationError = new Error(message, { cause: error })
              Object.assign(validationError, {
                validationError: true,
                validationAttempt: count,
                validationIssues: error.issues.map((issue) => ({ path: issue.path, message: issue.message })),
              })
              throw validationError
            }
            throw new Error(
              `The ${id} tool was called with invalid arguments: ${error}.\nPlease rewrite the input so it satisfies the expected schema.`,
              { cause: error },
            )
          }
          ToolValidationRetry.recordSuccess(ctx.sessionID, id)
          const run = await execute(args, ctx)
          const result = label ? { ...run, metadata: { ...run.metadata, label } } : run
          // skip truncation for tools that handle it themselves
          if (result.metadata.truncated !== undefined) {
            return result
          }
          const truncated = await Truncate.output(result.output, {}, initCtx?.agent)
          return {
            ...result,
            output: truncated.content,
            metadata: {
              ...result.metadata,
              truncated: truncated.truncated,
              ...(truncated.truncated && { outputPath: truncated.outputPath }),
            },
          }
        }
        return toolInfo
      },
    }
  }

  /** True when the schema spreads `LabelField` (checked by identity, so a tool's own `label` field is left alone). */
  function takesLabel(parameters: z.ZodType): boolean {
    return parameters instanceof z.ZodObject && (parameters.shape as Record<string, unknown>).label === LabelField.label
  }

  /** Split `label` off a call's args; returns it only when it passes `ToolCallLabel`. */
  function splitLabel<A>(raw: A): { args: A; label: string | undefined } {
    if (!raw || typeof raw !== "object" || Array.isArray(raw) || !("label" in raw)) return { args: raw, label: undefined }
    const { label, ...rest } = raw as Record<string, unknown>
    const parsed = ToolCallLabel.safeParse(label)
    return { args: rest as A, label: parsed.success ? parsed.data : undefined }
  }
}

// Merge-by-re-export: complete counterpart (local exports win on conflict)
export { buildTool, findToolByName, toolMatchesName } from "../../../Tool.js";
export type { AnyObject, Progress, SetToolJSXFn, ToolCall, ToolDef, ToolInvocation, ToolPermissionContext, ToolResult, ToolUse, ToolUseContext, Tools, ValidationResult } from "../../../Tool.js";