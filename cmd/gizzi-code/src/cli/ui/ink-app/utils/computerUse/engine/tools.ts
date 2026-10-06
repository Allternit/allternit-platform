/**
 * MCP tool definitions for the Computer Use engine adapter.
 *
 * Replaces `buildComputerUseTools` from `@ant/computer-use-mcp`. The tool
 * vocabulary mirrors what toolRendering.tsx renders and what session.ts
 * dispatches — keep the three in sync when adding tools.
 */

import type { Tool } from '@modelcontextprotocol/client'
import type {
  CoordinateMode,
  CuCapabilities,
} from './types.js'

export type CuToolSpec = Tool

function coordDescription(coordinateMode: CoordinateMode): string {
  return coordinateMode === 'normalized'
    ? 'Normalized (x, y) coordinates in a 0-1000 square, scaled to the active display.'
    : 'Absolute pixel (x, y) coordinates on the active display.'
}

function clickSpec(
  name: string,
  description: string,
  coordinateMode: CoordinateMode,
): CuToolSpec {
  return {
    name,
    description,
    inputSchema: {
      type: 'object',
      properties: {
        coordinate: {
          type: 'array',
          items: { type: 'number' },
          description: coordDescription(coordinateMode),
        },
        modifiers: {
          type: 'array',
          items: { type: 'string' },
          description:
            'Modifier keys held during the click, e.g. ["command", "shift"].',
        },
      },
      required: ['coordinate'],
    },
  }
}

export function buildComputerUseTools(
  capabilities: CuCapabilities,
  coordinateMode: CoordinateMode = 'pixels',
  installedAppNames?: string[],
): CuToolSpec[] {
  const installedHint =
    installedAppNames && installedAppNames.length > 0
      ? ` Installed apps include: ${installedAppNames.join(', ')}.`
      : ''

  return [
    {
      name: 'screenshot',
      description:
        'Take a screenshot of the active display. Returns an image of what the user sees.',
      inputSchema: { type: 'object', properties: {} },
    },
    {
      name: 'zoom',
      description:
        'Capture a zoomed crop of a screen region (engine backend: region capture may be unsupported; falls back with an error).',
      inputSchema: {
        type: 'object',
        properties: {
          region: {
            type: 'array',
            items: { type: 'number' },
            description: `[x, y, width, height] in ${coordinateMode === 'normalized' ? 'normalized' : 'pixel'} coordinates.`,
          },
        },
        required: ['region'],
      },
    },
    {
      name: 'request_access',
      description:
        `Request permission to control apps. Call before the first click/type/key/scroll/drag against a new app.${installedHint}`,
      inputSchema: {
        type: 'object',
        properties: {
          apps: {
            type: 'array',
            items: { type: 'string' },
            description: 'App names or bundle IDs to request access to.',
          },
          clipboardRead: { type: 'boolean' },
          clipboardWrite: { type: 'boolean' },
          systemKeyCombos: { type: 'boolean' },
        },
        required: ['apps'],
      },
    },
    clickSpec('left_click', 'Click the left mouse button.', coordinateMode),
    clickSpec('right_click', 'Click the right mouse button.', coordinateMode),
    clickSpec('middle_click', 'Click the middle mouse button.', coordinateMode),
    clickSpec(
      'double_click',
      'Double-click the left mouse button.',
      coordinateMode,
    ),
    clickSpec(
      'triple_click',
      'Triple-click the left mouse button.',
      coordinateMode,
    ),
    {
      name: 'left_click_drag',
      description: 'Press and drag the left mouse button.',
      inputSchema: {
        type: 'object',
        properties: {
          start_coordinate: {
            type: 'array',
            items: { type: 'number' },
            description: `Drag origin; omit to drag from the current cursor position. ${coordDescription(coordinateMode)}`,
          },
          coordinate: {
            type: 'array',
            items: { type: 'number' },
            description: `Drag destination. ${coordDescription(coordinateMode)}`,
          },
        },
        required: ['coordinate'],
      },
    },
    {
      name: 'type',
      description: 'Type text into the focused element.',
      inputSchema: {
        type: 'object',
        properties: {
          text: { type: 'string', description: 'Text to type.' },
          via_clipboard: {
            type: 'boolean',
            description:
              'Paste via the clipboard instead of per-key events (faster for long text).',
          },
        },
        required: ['text'],
      },
    },
    {
      name: 'key',
      description:
        'Press a key combination, e.g. "command+space" or "tab". Repeat for held repetition.',
      inputSchema: {
        type: 'object',
        properties: {
          keySequence: {
            type: 'string',
            description: 'xdotool-style key sequence, "+"-separated.',
          },
          repeat: { type: 'number', description: 'Repeat count (default 1).' },
        },
        required: ['keySequence'],
      },
    },
    {
      name: 'hold_key',
      description: 'Hold key(s) down for a duration, then release.',
      inputSchema: {
        type: 'object',
        properties: {
          keyNames: { type: 'array', items: { type: 'string' } },
          duration: { type: 'number', description: 'Hold time in ms.' },
        },
        required: ['keyNames', 'duration'],
      },
    },
    {
      name: 'scroll',
      description: 'Scroll the wheel at a position.',
      inputSchema: {
        type: 'object',
        properties: {
          coordinate: {
            type: 'array',
            items: { type: 'number' },
            description: coordDescription(coordinateMode),
          },
          delta_x: { type: 'number' },
          delta_y: { type: 'number' },
        },
        required: ['coordinate', 'delta_y'],
      },
    },
    {
      name: 'open_application',
      description: `Open or activate an application by name or bundle ID.${installedHint}`,
      inputSchema: {
        type: 'object',
        properties: {
          app: { type: 'string', description: 'App name or bundle ID.' },
        },
        required: ['app'],
      },
    },
    {
      name: 'switch_display',
      description:
        'Target a specific display, or "auto" to let the engine pick.',
      inputSchema: {
        type: 'object',
        properties: {
          display: {
            type: 'string',
            description: 'Display ID as a string, or "auto".',
          },
        },
        required: ['display'],
      },
    },
  ]
}

export type { CuCapabilities }
