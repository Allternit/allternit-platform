// Anthropic toolset drivers: subclasses of the SDK's abstract computer and browser
// toolsets (computer_toolset_20260801 / browser_toolset_20260801) that run every
// member on an Allternit hosted computer via POST /v1/computers/{id}/toolset.
import {
  BetaAbstractBrowserToolset20260801,
  BetaAbstractComputerToolset20260801,
  ToolError,
  type BetaBrowserMemberResult,
  type BetaBrowserState,
  type BetaBrowserToolsetOptions,
  type BetaComputerMemberResult,
  type BetaComputerToolsetOptions,
  type BetaToolsetCallContext,
} from "@anthropic-ai/sdk/helpers/beta/toolsets"
import {
  ApprovalRequiredError,
  resultImage,
  resultText,
  type AllternitComputers,
  type Approval,
  type ToolsetName,
  type ToolsetResult,
} from "./client.ts"
import { computerV2Tool, runComputerV2Member } from "./v2.ts"

export interface DriverOptions {
  client: AllternitComputers
  computerId: string
  /** Called when the server holds a call for approval. true → approve() and resend with the grant. */
  onApproval?: (approval: Approval) => Promise<boolean>
  /** run_id / turn_id / call_index for this call, if your loop tracks them. */
  callIds?: (ctx: BetaToolsetCallContext, member: string) => { run_id?: string; turn_id?: string; call_index?: number }
  browserSessionId?: string
}

/**
 * Default SDK `confirm`: let the call through. The Allternit server is the gate: it applies the
 * project's member policy and holds risky calls with a 409, which `onApproval` answers. Pass your
 * own `confirm` to also prompt locally before the call leaves the process.
 */
const serverDecides = () => true

async function callMember(o: DriverOptions, toolset: ToolsetName, ctx: BetaToolsetCallContext, member: string, input: unknown) {
  const call = {
    toolset,
    member,
    input: (input ?? {}) as Record<string, unknown>,
    // The SDK only calls members its toolset config turned on, which is the
    // opt-in the executor asks for on off-by-default members.
    enable: [member],
    ...(o.callIds?.(ctx, member) ?? {}),
    ...(o.browserSessionId ? { browser_session_id: o.browserSessionId } : {}),
  }
  let res: ToolsetResult
  try {
    res = await o.client.toolset(o.computerId, call)
  } catch (e) {
    if (!(e instanceof ApprovalRequiredError)) throw e
    if (!o.onApproval || !(await o.onApproval(e.approval))) res = e.result
    else {
      await o.client.approve(o.computerId, e.approval.id)
      res = await o.client.toolset(o.computerId, { ...call, approval_grant: e.approval.id })
    }
  }
  if (res.is_error) throw new ToolError(toAnthropicBlocks(res))
  return res
}

function toAnthropicBlocks(r: ToolsetResult): any[] {
  const blocks = r.content.map((b) =>
    b.type === "text"
      ? { type: "text", text: b.text }
      : { type: "image", source: { type: "base64", media_type: b.media_type, data: b.data } },
  )
  return blocks.length ? blocks : [{ type: "text", text: "The action failed." }]
}

function shot(r: ToolsetResult) {
  const img = resultImage(r)
  if (!img) throw new ToolError("The computer returned no screenshot.")
  return { data: img.data, mediaType: img.media_type as "image/png" }
}

function textOrVoid(r: ToolsetResult): string | void {
  const t = resultText(r)
  return t ? t : undefined
}

function jsonOf(r: ToolsetResult): any {
  try {
    return JSON.parse(resultText(r))
  } catch {
    return undefined
  }
}

export class AllternitComputerToolset extends BetaAbstractComputerToolset20260801 {
  readonly #o: DriverOptions
  constructor(o: DriverOptions & BetaComputerToolsetOptions) {
    const { client: _c, computerId: _i, onApproval: _a, callIds: _d, browserSessionId: _b, ...sdk } = o
    super({ ...sdk, confirm: sdk.confirm ?? serverDecides })
    this.#o = o
  }

  protected override async execute(ctx: BetaToolsetCallContext, name: string, input: unknown): Promise<BetaComputerMemberResult> {
    const r = await callMember(this.#o, "computer", ctx, name, input)
    if (name === "screenshot" || name === "zoom") return shot(r)
    if (name === "cursor_position") {
      const j = jsonOf(r)
      if (j && typeof j.x === "number") return { x: j.x, y: j.y }
      const m = /(-?\d+)\D+(-?\d+)/.exec(resultText(r))
      if (!m) throw new ToolError("The computer returned no cursor position.")
      return { x: Number(m[1]), y: Number(m[2]) }
    }
    return textOrVoid(r)
  }
}

export class AllternitBrowserToolset extends BetaAbstractBrowserToolset20260801 {
  readonly #o: DriverOptions
  #state: BetaBrowserState | undefined
  constructor(o: DriverOptions & Omit<BetaBrowserToolsetOptions, "browserState"> & Partial<Pick<BetaBrowserToolsetOptions, "browserState">>) {
    const { client: _c, computerId: _i, onApproval: _a, callIds: _d, browserSessionId: _b, ...sdk } = o
    const self: { t?: AllternitBrowserToolset } = {}
    super({ ...sdk, confirm: sdk.confirm ?? serverDecides, browserState: sdk.browserState ?? ((ctx) => self.t!.#currentState(ctx)) })
    this.#o = o
    self.t = this
  }

  async #currentState(ctx: BetaToolsetCallContext): Promise<BetaBrowserState> {
    if (this.#state) return this.#state
    const r = await callMember(this.#o, "browser", ctx, "list_tabs", {})
    return (this.#state = (r.browser_state as BetaBrowserState) ?? { tabs: jsonOf(r) ?? [] })
  }

  protected override async execute(ctx: BetaToolsetCallContext, name: string, input: unknown): Promise<BetaBrowserMemberResult> {
    const r = await callMember(this.#o, "browser", ctx, name, input)
    if (r.browser_state) this.#state = r.browser_state as BetaBrowserState
    if (name === "screenshot" || name === "zoom") return shot(r)
    const j = jsonOf(r)
    const tabs = this.#state?.tabs ?? []
    switch (name) {
      case "navigate":
        return j?.url ? j : { url: tabs[0]?.url ?? String((input as { url?: string })?.url ?? "") }
      case "list_tabs":
        return Array.isArray(j) ? j : tabs
      case "new_tab":
      case "switch_tab":
        if (j?.tab_id) return j
        if (tabs[0]) return tabs[0]
        throw new ToolError("The browser returned no tab.")
      case "close_tab":
        return undefined
      default:
        return textOrVoid(r)
    }
  }
}

// ------------------------------------------------------------------ computer_v2

/**
 * The `computer_v2` function tool for Anthropic models: the ten structured
 * members (read_ui … skills) next to the native pixel toolset. The Anthropic
 * wire hook only rewrites tools carrying the v1 pixel marker, so this passes
 * through untouched.
 */
export function computerV2AnthropicTool(): { name: string; description: string; input_schema: Record<string, unknown> } {
  const t = computerV2Tool()
  return { name: t.name, description: t.description, input_schema: t.schema }
}

/**
 * Run one `computer_v2` function call (`{action, ...fields}`) and return the
 * tool-result content blocks, like any toolset member. Throws ToolError on a
 * failed action; without `onApproval`, a held call (409 approval_required)
 * resolves its held result as an error, the same rule as the toolsets.
 */
export async function runAnthropicComputerV2(o: DriverOptions, name: string, input: unknown): Promise<any[]> {
  const { action, ...fields } = (input ?? {}) as { action?: string } & Record<string, unknown>
  if (!action) throw new ToolError("The computer_v2 call needs an action.")
  const res = await runComputerV2Member({ client: o.client, computerId: o.computerId, onApproval: o.onApproval }, action, fields)
  if (res.is_error) throw new ToolError(toAnthropicBlocks(res))
  return toAnthropicBlocks(res)
}
