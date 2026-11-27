/**
 * request_signature in the Gizzi TUI: a thin adapter over the runtime builtin
 * (runtime/tools/builtins/request-signature.ts). The ink-app has its own tool
 * objects, so this one runs the runtime tool and the card in UI.tsx draws its
 * metadata. Nothing here signs, approves or declines.
 */
import { z } from 'zod/v4'
import { parameters, RequestSignatureTool as RuntimeRequestSignatureTool, type Metadata } from '@/runtime/tools/builtins/request-signature'
import { buildTool, type ToolDef } from '../../Tool.js'
import { lazySchema } from '../../utils/lazySchema.js'
import { DESCRIPTION, PROMPT, REQUEST_SIGNATURE_TOOL_NAME } from './prompt.js'
import { renderToolResultMessage, renderToolUseMessage } from './UI.js'

const inputSchema = lazySchema(() => parameters.strict())
type InputSchema = ReturnType<typeof inputSchema>

const outputSchema = lazySchema(() =>
  z.object({
    title: z.string(),
    output: z.string(),
    id: z.string(),
    status: z.string(),
    sign_url: z.string(),
    doc_title: z.string().optional(),
    signer_name: z.string().optional(),
    resolved_at: z.string().optional(),
  }),
)
type OutputSchema = ReturnType<typeof outputSchema>

export type Output = z.infer<OutputSchema>

export const RequestSignatureTool = buildTool({
  name: REQUEST_SIGNATURE_TOOL_NAME,
  searchHint: 'ask the user to sign a PDF',
  maxResultSizeChars: 10_000,
  userFacingName: () => 'Signature request',
  get inputSchema(): InputSchema {
    return inputSchema()
  },
  get outputSchema(): OutputSchema {
    return outputSchema()
  },
  isConcurrencySafe() {
    return false
  },
  toAutoClassifierInput(input) {
    return input.title ?? input.id ?? ''
  },
  async description() {
    return DESCRIPTION
  },
  async prompt() {
    return PROMPT
  },
  mapToolResultToToolResultBlockParam(output, toolUseID) {
    return { tool_use_id: toolUseID, type: 'tool_result', content: output.output }
  },
  renderToolUseMessage,
  renderToolResultMessage,
  async call(input, context) {
    const sessionID = String(context.agentId ?? 'tui')
    const tool = await RuntimeRequestSignatureTool.init()
    const out = await tool.execute(input, {
      sessionID,
      messageID: sessionID,
      agent: 'gizzi',
      abort: context.abortController.signal,
      extra: {},
      messages: [],
      metadata() {},
      // The runtime asks before it uploads the PDF; the TUI's own tool permission check has already run.
      async ask() {},
    })
    const m = out.metadata as Metadata
    return {
      data: {
        title: out.title,
        output: out.output,
        id: m.id,
        status: m.status,
        sign_url: m.sign_url,
        doc_title: m.title,
        signer_name: m.signer_name,
        resolved_at: m.resolved_at,
      },
    }
  },
} satisfies ToolDef<InputSchema, Output>)
