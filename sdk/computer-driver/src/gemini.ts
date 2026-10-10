// Gemini computer_use adapter: runs one predefined function call on an Allternit
// hosted computer. Gemini coordinates are 0–999 normalized, so every call is sent
// with coordinate_space "normalized_1000" and the server scales to the screen.
import { resultImage, resultText, type AllternitComputers, type Approval, type ToolsetCall, type ToolsetResult } from "./client.ts"
import { computerV2Tool, runComputerV2Member } from "./v2.ts"

export interface GeminiFunctionCall {
  name: string
  args?: Record<string, unknown>
}

export interface GeminiAdapterOptions {
  client: AllternitComputers
  computerId: string
  /** URL used by the `search` function. Default https://www.google.com. */
  searchUrl?: string
}

type Step = Pick<ToolsetCall, "member" | "input">
const pt = (x: unknown, y: unknown) => [Math.round(Number(x)), Math.round(Number(y))]
const key = (text: string): Step => ({ member: "key", input: { text } })
const typed = (text: string): Step => ({ member: "type", input: { text } })
const goTo = (url: string): Step[] => [key("ctrl+l"), typed(url), key("Return")]

/** "Control+Shift+T" → "ctrl+shift+t". */
export function geminiKeys(keys: string): string {
  const map: Record<string, string> = { control: "ctrl", meta: "super", command: "super", enter: "Return", escape: "Escape", backspace: "BackSpace", tab: "Tab", delete: "Delete" }
  return keys.split("+").map((k) => k.trim()).map((k) => map[k.toLowerCase()] ?? (k.length === 1 ? k.toLowerCase() : k)).join("+")
}

/** Contract calls for one Gemini computer_use function call, in order. */
export function geminiToCalls(fc: GeminiFunctionCall, searchUrl = "https://www.google.com"): Step[] {
  const a = fc.args ?? {}
  const dir = String(a.direction ?? "down")
  switch (fc.name) {
    case "open_web_browser":
      return []
    case "wait_5_seconds":
      return [{ member: "wait", input: { duration: 5 } }]
    case "go_back":
      return [key("alt+Left")]
    case "go_forward":
      return [key("alt+Right")]
    case "search":
      return goTo(searchUrl)
    case "navigate":
      return goTo(String(a.url ?? ""))
    case "click_at":
      return [{ member: "left_click", input: { coordinate: pt(a.x, a.y) } }]
    case "hover_at":
      return [{ member: "mouse_move", input: { coordinate: pt(a.x, a.y) } }]
    case "type_text_at": {
      const steps: Step[] = [{ member: "left_click", input: { coordinate: pt(a.x, a.y) } }]
      if (a.clear_before_typing !== false) steps.push(key("ctrl+a"), key("BackSpace"))
      steps.push(typed(String(a.text ?? "")))
      if (a.press_enter !== false) steps.push(key("Return"))
      return steps
    }
    case "key_combination":
      return [key(geminiKeys(String(a.keys ?? "")))]
    case "scroll_document":
      return [{ member: "scroll", input: { coordinate: [500, 500], scroll_direction: dir, scroll_amount: 5 } }]
    case "scroll_at": {
      const amount = Math.max(1, Math.round(Number(a.magnitude ?? 800) / 160))
      return [{ member: "scroll", input: { coordinate: pt(a.x, a.y), scroll_direction: dir, scroll_amount: amount } }]
    }
    case "drag_and_drop":
      return [{ member: "left_click_drag", input: { start_coordinate: pt(a.x, a.y), coordinate: pt(a.destination_x, a.destination_y) } }]
    default:
      throw new Error(`Unsupported Gemini computer_use function: ${fc.name}`)
  }
}

export interface GeminiStepResult {
  /** A `functionResponse` part for the next turn. */
  functionResponse: { name: string; response: Record<string, unknown> }
  /** The screenshot after the action, as an `inlineData` part. */
  inlineData: { mimeType: string; data: string }
  error?: ToolsetResult
}

/** Run one Gemini function call and return the parts Gemini expects back. */
export async function runGeminiCall(o: GeminiAdapterOptions, fc: GeminiFunctionCall): Promise<GeminiStepResult> {
  const base = { toolset: "computer" as const, coordinate_space: "normalized_1000" as const }
  let error: ToolsetResult | undefined
  for (const step of geminiToCalls(fc, o.searchUrl)) {
    const r = await o.client.toolset(o.computerId, { ...base, ...step })
    if (r.is_error) {
      error = r
      break
    }
  }
  const s = await o.client.toolset(o.computerId, { ...base, member: "screenshot", input: {} })
  const img = resultImage(s)
  if (!img) throw new Error("Allternit computer returned no screenshot.")
  const response: Record<string, unknown> = { url: "" }
  if (error) response.error = error.content.flatMap((b) => (b.type === "text" ? [b.text] : [])).join("\n")
  return {
    functionResponse: { name: fc.name, response },
    inlineData: { mimeType: img.media_type, data: img.data },
    ...(error ? { error } : {}),
  }
}

// ------------------------------------------------------------------ computer_v2

/**
 * The `computer_v2` function declaration for Gemini models: the ten structured
 * members next to the `computer_use` declaration. Route matching
 * `functionCall` parts through runGeminiV2Call.
 */
export function geminiComputerV2Declaration(): { name: string; description: string; parameters: Record<string, unknown> } {
  const t = computerV2Tool()
  return { name: t.name, description: t.description, parameters: t.schema }
}

export interface GeminiV2AdapterOptions extends GeminiAdapterOptions {
  /** Called when the server holds the call (409 approval_required). Without it the ApprovalRequiredError propagates, like the pixel adapter. */
  onApproval?: (approval: Approval) => boolean | Promise<boolean>
}

export interface GeminiV2StepResult {
  /** A `functionResponse` part for the next turn. */
  functionResponse: { name: string; response: Record<string, unknown> }
  error?: ToolsetResult
}

/** Run one `computer_v2` function call (`{action, ...fields}`) and return the function response part. */
export async function runGeminiV2Call(o: GeminiV2AdapterOptions, name: string, input: unknown): Promise<GeminiV2StepResult> {
  const { action, ...fields } = (input ?? {}) as { action?: string } & Record<string, unknown>
  if (!action) throw new Error("The computer_v2 call needs an action.")
  const base = { client: o.client, computerId: o.computerId }
  const res = o.onApproval
    ? await runComputerV2Member({ ...base, onApproval: o.onApproval }, action, fields)
    : await o.client.toolset(o.computerId, { toolset: "computer", member: action, input: fields })
  const text = resultText(res)
  const response: Record<string, unknown> = { result: text }
  if (res.is_error) response.error = text || "The action failed."
  return { functionResponse: { name, response }, ...(res.is_error ? { error: res } : {}) }
}
