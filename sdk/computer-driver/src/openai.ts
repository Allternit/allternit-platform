// OpenAI computer-use adapter: runs one `computer_call` action on an Allternit
// hosted computer and returns a `computer_call_output` with the next screenshot.
import { resultImage, type AllternitComputers, type Approval, type ToolsetCall, type ToolsetResult } from "./client.ts"
import { computerV2Tool, runComputerV2Member } from "./v2.ts"

type Pt = { x: number; y: number }
export type OpenAIComputerAction =
  | { type: "click"; x: number; y: number; button?: "left" | "right" | "wheel" | "back" | "forward" }
  | { type: "double_click"; x: number; y: number }
  | { type: "drag"; path: Pt[] }
  | { type: "keypress"; keys: string[] }
  | { type: "move"; x: number; y: number }
  | { type: "scroll"; x: number; y: number; scroll_x: number; scroll_y: number }
  | { type: "type"; text: string }
  | { type: "wait"; ms?: number }
  | { type: "screenshot" }

export interface OpenAIAdapterOptions {
  client: AllternitComputers
  computerId: string
  /** display_width / display_height you declared on the computer_use_preview tool. */
  display?: { width: number; height: number }
  /** Pixels of scroll per wheel click when converting scroll_x / scroll_y. Default 100. */
  pixelsPerScrollClick?: number
}

const KEYS: Record<string, string> = {
  CTRL: "ctrl", CONTROL: "ctrl", ALT: "alt", OPTION: "alt", SHIFT: "shift", CMD: "super", META: "super", SUPER: "super",
  WIN: "super", ENTER: "Return", RETURN: "Return", ESC: "Escape", ESCAPE: "Escape", TAB: "Tab", SPACE: "space",
  BACKSPACE: "BackSpace", DELETE: "Delete", UP: "Up", DOWN: "Down", LEFT: "Left", RIGHT: "Right",
  ARROWUP: "Up", ARROWDOWN: "Down", ARROWLEFT: "Left", ARROWRIGHT: "Right", HOME: "Home", END: "End",
  PAGEUP: "Page_Up", PAGEDOWN: "Page_Down",
}

/** ["CTRL","C"] → "ctrl+c" (the contract's xdotool-style key text). */
export function openaiKeys(keys: string[]): string {
  return keys.map((k) => KEYS[k.toUpperCase()] ?? (k.length === 1 ? k.toLowerCase() : k)).join("+")
}

/** Contract calls for one OpenAI action, in order. */
export function openaiToCalls(a: OpenAIComputerAction, perClick = 100): Array<Pick<ToolsetCall, "member" | "input">> {
  const c = (x: number, y: number) => [Math.round(x), Math.round(y)]
  switch (a.type) {
    case "click": {
      const b = a.button ?? "left"
      if (b === "back") return [{ member: "key", input: { text: "alt+Left" } }]
      if (b === "forward") return [{ member: "key", input: { text: "alt+Right" } }]
      const member = b === "right" ? "right_click" : b === "wheel" ? "middle_click" : "left_click"
      return [{ member, input: { coordinate: c(a.x, a.y) } }]
    }
    case "double_click":
      return [{ member: "double_click", input: { coordinate: c(a.x, a.y) } }]
    case "drag": {
      const p = a.path
      if (p.length < 2) return []
      if (p.length === 2) return [{ member: "left_click_drag", input: { start_coordinate: c(p[0].x, p[0].y), coordinate: c(p[1].x, p[1].y) } }]
      return [
        { member: "mouse_move", input: { coordinate: c(p[0].x, p[0].y) } },
        { member: "left_mouse_down", input: {} },
        ...p.slice(1).map((q) => ({ member: "mouse_move", input: { coordinate: c(q.x, q.y) } })),
        { member: "left_mouse_up", input: {} },
      ]
    }
    case "keypress":
      return [{ member: "key", input: { text: openaiKeys(a.keys) } }]
    case "move":
      return [{ member: "mouse_move", input: { coordinate: c(a.x, a.y) } }]
    case "scroll": {
      const out: Array<Pick<ToolsetCall, "member" | "input">> = []
      const amt = (v: number) => Math.max(1, Math.round(Math.abs(v) / perClick))
      if (a.scroll_y) out.push({ member: "scroll", input: { coordinate: c(a.x, a.y), scroll_direction: a.scroll_y > 0 ? "down" : "up", scroll_amount: amt(a.scroll_y) } })
      if (a.scroll_x) out.push({ member: "scroll", input: { coordinate: c(a.x, a.y), scroll_direction: a.scroll_x > 0 ? "right" : "left", scroll_amount: amt(a.scroll_x) } })
      return out
    }
    case "type":
      return [{ member: "type", input: { text: a.text } }]
    case "wait":
      return [{ member: "wait", input: { duration: Math.max(1, Math.round((a.ms ?? 2000) / 1000)) } }]
    case "screenshot":
      return []
  }
}

export interface OpenAIStepResult {
  /** Ready for the Responses API `input`: `{type:"computer_call_output", call_id, output}`. */
  output: { type: "computer_call_output"; call_id: string; output: { type: "computer_screenshot"; image_url: string } }
  /** The last failed action result, if any action failed. */
  error?: ToolsetResult
}

/** Run one OpenAI `computer_call` (its `call_id` and `action`) and return the output item. */
export async function runOpenAIAction(o: OpenAIAdapterOptions, callId: string, action: OpenAIComputerAction): Promise<OpenAIStepResult> {
  const frame = o.display ? { model_frame: o.display, coordinate_space: "pixels" as const } : {}
  let error: ToolsetResult | undefined
  for (const step of openaiToCalls(action, o.pixelsPerScrollClick)) {
    const r = await o.client.toolset(o.computerId, { toolset: "computer", ...step, ...frame })
    if (r.is_error) {
      error = r
      break
    }
  }
  const s = await o.client.toolset(o.computerId, { toolset: "computer", member: "screenshot", input: {}, ...frame })
  const img = resultImage(s)
  if (!img) throw new Error("Allternit computer returned no screenshot.")
  return {
    output: {
      type: "computer_call_output",
      call_id: callId,
      output: { type: "computer_screenshot", image_url: `data:${img.media_type};base64,${img.data}` },
    },
    ...(error ? { error } : {}),
  }
}

// ------------------------------------------------------------------ computer_v2

/**
 * The `computer_v2` function tool for the Responses API: the ten structured
 * members next to the `computer_use_preview` tool. Register it in `tools`;
 * route matching `function_call` items through runOpenAIV2Call.
 */
export function openaiComputerV2Tool(): { type: "function"; name: string; description: string; parameters: Record<string, unknown>; strict: false } {
  const t = computerV2Tool()
  return { type: "function", name: t.name, description: t.description, parameters: t.schema, strict: false }
}

export interface OpenAIV2AdapterOptions extends OpenAIAdapterOptions {
  /** Called when the server holds the call (409 approval_required). Without it the ApprovalRequiredError propagates, like the pixel adapter. */
  onApproval?: (approval: Approval) => boolean | Promise<boolean>
}

export interface OpenAIV2StepResult {
  /** Ready for the Responses API `input`: `{type:"function_call_output", call_id, output}`. */
  output: { type: "function_call_output"; call_id: string; output: string }
  /** The failed action result, if the action failed. */
  error?: ToolsetResult
}

/** Run one `computer_v2` function call (`{action, ...fields}`) and return the `function_call_output` item. */
export async function runOpenAIV2Call(o: OpenAIV2AdapterOptions, callId: string, input: unknown): Promise<OpenAIV2StepResult> {
  const { action, ...fields } = (input ?? {}) as { action?: string } & Record<string, unknown>
  if (!action) throw new Error("The computer_v2 call needs an action.")
  const base = { client: o.client, computerId: o.computerId }
  const res = o.onApproval
    ? await runComputerV2Member({ ...base, onApproval: o.onApproval }, action, fields)
    : await o.client.toolset(o.computerId, { toolset: "computer", member: action, input: fields })
  const text = res.content.flatMap((b) => (b.type === "text" ? [b.text] : [])).join("\n")
  return {
    output: { type: "function_call_output", call_id: callId, output: text || "Done." },
    ...(res.is_error ? { error: res } : {}),
  }
}
