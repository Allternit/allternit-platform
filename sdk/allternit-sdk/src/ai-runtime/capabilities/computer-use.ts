import type { ToolDefinition } from '../tools/types.js';

/** Allternit Computer Use tool versions. The action sets mirror the upstream
 * `computer_20250124` / `computer_20251124` shapes that models are trained on. */
export type ComputerToolVersion = '20250124' | '20251124';

export const COMPUTER_20250124_ACTIONS = [
  'key', 'type', 'mouse_move', 'left_click', 'left_click_drag',
  'right_click', 'middle_click', 'double_click', 'triple_click',
  'left_mouse_down', 'left_mouse_up', 'screenshot', 'cursor_position',
  'scroll', 'hold_key', 'wait',
] as const;

/** The 20251124 action set adds `zoom`; the tool must also be created with
 * `enableZoom: true` so the model is allowed to emit that action. */
export const COMPUTER_20251124_ACTIONS = [...COMPUTER_20250124_ACTIONS, 'zoom'] as const;

export type ComputerUseAction =
  | (typeof COMPUTER_20250124_ACTIONS)[number]
  | (typeof COMPUTER_20251124_ACTIONS)[number];

export interface ComputerUseInput {
  action: ComputerUseAction;
  text?: string;
  coordinate?: [number, number];
  /** 20251124 only: [x1, y1, x2, y2] pixel region for the "zoom" action. */
  region?: [number, number, number, number];
  scroll_direction?: 'up' | 'down' | 'left' | 'right';
  scroll_amount?: number;
  duration?: number;
}

export interface ComputerUseOptions {
  gatewayUrl?: string;
  fetch?: typeof globalThis.fetch;
  displayWidthPx?: number;
  displayHeightPx?: number;
  displayNumber?: number;
  /** Action-set version. Defaults to '20250124'. */
  toolVersion?: ComputerToolVersion;
  /** 20251124 only: advertise the "zoom" action and emit `enable_zoom` metadata. */
  enableZoom?: boolean;
  /**
   * Asked before every risky or irreversible action (clicks, drags, typing
   * that submits, Enter). Without one, those actions are refused: the tool
   * never acts on a person's computer without a human decision.
   */
  approve?: ComputerUseApprover;
}

/**
 * How much human confirmation an action needs. The same three classes as
 * allternit-api's `aci_safety::ConfirmationClass`, so the SDK and the server
 * agree on what counts as risky.
 */
export type ComputerActionClass = 'reversible' | 'risky' | 'irreversible';

export interface ComputerUseApprovalRequest {
  action: ComputerUseAction;
  actionClass: Exclude<ComputerActionClass, 'reversible'>;
  input: ComputerUseInput;
  context: unknown;
}

/** Return true (or `{ approved: true }`) to let the action run. */
export type ComputerUseApprover = (
  request: ComputerUseApprovalRequest,
) => Promise<boolean | { approved: boolean; reason?: string }>;

const REVERSIBLE_ACTIONS = new Set<string>([
  'screenshot', 'cursor_position', 'mouse_move', 'scroll', 'wait', 'zoom',
]);

/** Classify one computer action (mirrors `aci_safety` mouse/keyboard rules). */
export function classifyComputerAction(input: Pick<ComputerUseInput, 'action' | 'text'>): ComputerActionClass {
  const action = String(input.action ?? '');
  if (REVERSIBLE_ACTIONS.has(action)) return 'reversible';
  if (action === 'type') {
    // A newline usually submits the focused form.
    return typeof input.text === 'string' && !/[\r\n]/.test(input.text) ? 'reversible' : 'risky';
  }
  if (action === 'key' || action === 'hold_key') {
    const key = typeof input.text === 'string' ? input.text.toLowerCase() : '';
    if (!key) return 'risky';
    return key.includes('return') || key.includes('enter') ? 'risky' : 'reversible';
  }
  // Clicks, drags, button down/up and anything unknown activate whatever is
  // under the cursor: risky, fail-safe.
  return 'risky';
}

/**
 * The `preExecute` gate for the computer tool: reversible actions run,
 * risky and irreversible ones need `approve` to say yes. No approver means
 * deny.
 */
export function createComputerPreExecute(approve?: ComputerUseApprover): NonNullable<ToolDefinition['preExecute']> {
  return async (args: ComputerUseInput, context: unknown) => {
    const actionClass = classifyComputerAction(args ?? ({} as ComputerUseInput));
    if (actionClass === 'reversible') return { proceed: true };
    if (!approve) {
      return {
        proceed: false,
        reason: `Computer action "${String(args?.action)}" is ${actionClass} and needs a person's approval, but no approver is configured.`,
      };
    }
    try {
      const verdict = await approve({ action: args.action, actionClass, input: args, context });
      const approved = typeof verdict === 'boolean' ? verdict : verdict?.approved === true;
      if (approved) return { proceed: true };
      const reason = typeof verdict === 'object' && verdict?.reason ? `: ${verdict.reason}` : '';
      return { proceed: false, reason: `Computer action "${String(args.action)}" was not approved${reason}.` };
    } catch (error) {
      return {
        proceed: false,
        reason: `Computer action "${String(args.action)}" approval failed: ${error instanceof Error ? error.message : String(error)}`,
      };
    }
  };
}

function buildComputerInputSchema(version: ComputerToolVersion): ToolDefinition['input_schema'] {
  const actions = version === '20251124' ? COMPUTER_20251124_ACTIONS : COMPUTER_20250124_ACTIONS;
  return {
    type: 'object',
    properties: {
      action: {
        type: 'string',
        enum: [...actions],
        description: 'The computer action to perform'
      },
      text: { type: 'string', description: 'Text to type for the "type" and "key" actions' },
      coordinate: {
        type: 'array',
        items: { type: 'number' },
        minItems: 2,
        maxItems: 2,
        description: 'The absolute [x, y] pixel coordinates for mouse actions'
      },
      ...(version === '20251124' ? {
        region: {
          type: 'array',
          items: { type: 'number' },
          minItems: 4,
          maxItems: 4,
          description: 'The absolute [x1, y1, x2, y2] pixel region for the "zoom" action (top-left and bottom-right corners)'
        },
      } : {}),
      scroll_direction: { type: 'string', enum: ['up', 'down', 'left', 'right'], description: 'Direction for scroll' },
      scroll_amount: { type: 'integer', description: 'Number of scroll ticks' },
      duration: { type: 'number', description: 'Duration in seconds for hold_key and wait' },
    },
    required: ['action']
  };
}

function extractScreenshotPayload(data: Record<string, unknown>): { data: string; mediaType: string } | undefined {
  // Gateway canonical shape: screenshot lives in artifacts[] as
  // { type: 'screenshot', mime?, content? | url? }, where content/url may be a
  // raw base64 string or a data: URL.
  const artifacts = Array.isArray(data.artifacts) ? data.artifacts as Record<string, unknown>[] : [];
  for (const artifact of artifacts) {
    if (!artifact || artifact.type !== 'screenshot') continue;
    const raw = typeof artifact.content === 'string'
      ? artifact.content
      : typeof artifact.url === 'string' ? artifact.url : undefined;
    if (!raw) continue;
    const mediaType = typeof artifact.mime === 'string'
      ? artifact.mime
      : typeof data.media_type === 'string' ? data.media_type : 'image/png';
    const match = /^data:([^;]+);base64,(.*)$/.exec(raw);
    return { data: match ? match[2] : raw, mediaType: match ? match[1] : mediaType };
  }
  // Legacy fallback: gateway versions that returned top-level screenshot/data.
  const legacy = typeof data.screenshot === 'string'
    ? data.screenshot
    : typeof data.data === 'string' ? data.data : undefined;
  if (!legacy) return undefined;
  const mediaType = typeof data.media_type === 'string' ? data.media_type : 'image/png';
  const match = /^data:([^;]+);base64,(.*)$/.exec(legacy);
  return { data: match ? match[2] : legacy, mediaType: match ? match[1] : mediaType };
}

export const COMPUTER_USE_TOOL: ToolDefinition = {
  name: 'computer',
  description: 'Control the mouse and keyboard, and capture screenshots to interact with the computer.',
  input_schema: buildComputerInputSchema('20250124'),
  metadata: {
    category: 'vision',
    isDestructive: true,
    requiresVision: true,
    // Vendor-neutral Allternit branding (design decision D3). `anthropicType`
    // is the legacy upstream-compat adapter, kept during the transition.
    allternitToolType: 'computer',
    computerToolVersion: '20250124',
    anthropicType: 'computer_20250124',
    display_width_px: 1024,
    display_height_px: 768,
  },
  // No approver on the shared definition: risky actions are refused.
  preExecute: createComputerPreExecute(),
};

/**
 * @deprecated since 2026-10-09. Use `@allternit/computer-driver`
 * (`@allternit/computer-driver/anthropic` for the Anthropic computer tool),
 * which runs each action on a hosted computer's toolset executor. This class
 * still posts to the local ACU gateway `/v1/execute` and is kept as a thin
 * shim for existing ai-runtime callers.
 */
export class ComputerUseCapability {
  private readonly gatewayUrl: string;
  private readonly fetchImpl: typeof globalThis.fetch;
  private readonly displayWidthPx: number;
  private readonly displayHeightPx: number;
  private readonly displayNumber?: number;
  private readonly toolVersion: ComputerToolVersion;
  private readonly enableZoom: boolean;
  private readonly approve?: ComputerUseApprover;

  constructor(options: string | ComputerUseOptions = {}) {
    const normalized = typeof options === 'string' ? { gatewayUrl: options } : options;
    this.gatewayUrl = normalized.gatewayUrl || process.env.ALLTERNIT_COMPUTER_USE_URL || process.env.Allternit_COMPUTER_USE_URL || 'http://127.0.0.1:8760';
    this.fetchImpl = normalized.fetch ?? globalThis.fetch;
    this.displayWidthPx = normalized.displayWidthPx ?? 1024;
    this.displayHeightPx = normalized.displayHeightPx ?? 768;
    this.displayNumber = normalized.displayNumber;
    this.toolVersion = normalized.toolVersion ?? '20250124';
    this.enableZoom = normalized.enableZoom ?? false;
    this.approve = normalized.approve;
  }

  public getTool(): ToolDefinition {
    return {
      ...COMPUTER_USE_TOOL,
      input_schema: buildComputerInputSchema(this.toolVersion),
      metadata: {
        ...COMPUTER_USE_TOOL.metadata,
        allternitToolType: 'computer',
        computerToolVersion: this.toolVersion,
        anthropicType: `computer_${this.toolVersion}`,
        display_width_px: this.displayWidthPx,
        display_height_px: this.displayHeightPx,
        ...(this.displayNumber === undefined ? {} : { display_number: this.displayNumber }),
        ...(this.toolVersion === '20251124' && this.enableZoom ? { enable_zoom: true } : {}),
      },
      preExecute: createComputerPreExecute(this.approve),
      execute: this.execute.bind(this)
    };
  }

  public async execute(args: ComputerUseInput): Promise<string | Array<Record<string, unknown>>> {
    try {
      const response = await this.fetchImpl(`${this.gatewayUrl}/v1/execute`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          action: args.action,
          parameters: {
            text: args.text,
            coordinate: args.coordinate,
            region: args.region,
            scroll_direction: args.scroll_direction,
            scroll_amount: args.scroll_amount,
            duration: args.duration,
          },
          family: 'desktop'
        })
      });

      if (!response.ok) {
        const detail = await response.text().catch(() => '');
        throw new Error(
          `Computer Use gateway HTTP ${response.status}${response.statusText ? ` ${response.statusText}` : ''}${detail ? `: ${detail.slice(0, 300)}` : ''}`,
        );
      }

      const data = await response.json() as Record<string, unknown>;

      // Never fake success. The gateway reports failures in-band
      // (status: 'failed' and/or error: {code, message}) with HTTP 200, so
      // a resolved fetch does not mean the action ran. Surface the error
      // and any partial state (summary) the gateway did produce.
      const gatewayError = data.error as
        | { code?: string; message?: string }
        | string
        | null
        | undefined;
      if (data.status === 'failed' || gatewayError) {
        const message =
          typeof gatewayError === 'string'
            ? gatewayError
            : (gatewayError?.message ?? 'unknown gateway error');
        const code =
          gatewayError && typeof gatewayError === 'object' && gatewayError.code
            ? ` (${gatewayError.code})`
            : '';
        const partial =
          typeof data.summary === 'string' && data.summary
            ? ` Partial state: ${data.summary}`
            : '';
        return `Error executing computer action${code}: ${message}.${partial}`;
      }

      if (args.action === 'screenshot') {
        const screenshot = extractScreenshotPayload(data);
        if (!screenshot) throw new Error('Computer Use gateway returned no screenshot data');
        return [{
          type: 'image',
          source: {
            type: 'base64',
            media_type: screenshot.mediaType,
            data: screenshot.data,
          },
        }];
      }
      // Only a completed run may report success; anything else is surfaced
      // as an error rather than falling through to a canned completion line.
      if (data.status !== 'completed') {
        return `Error executing computer action: gateway run ended with status '${String(data.status)}'.`;
      }
      return typeof data.summary === 'string' && data.summary
        ? data.summary
        : `Action ${args.action} completed.`;
    } catch (error) {
      return `Error executing computer action: ${error instanceof Error ? error.message : String(error)}`;
    }
  }
}
