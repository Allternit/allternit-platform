/**
 * Keybindings template generator.
 * Generates a well-documented template file for ~/.gizzi/keybindings.json
 */

import { jsonStringify } from '../utils/slowOperations.js'
import { DEFAULT_BINDINGS } from './defaultBindings.js'
import {
  NON_REBINDABLE,
  normalizeKeyForComparison,
} from './reservedShortcuts.js'
// TODO(types): './types.js' (keybindings/types.ts) is a dormant stub
// (types_ts()) exporting nothing. Local mirror of the shape this file
// needs — same pattern as defaultBindings.ts.
type KeybindingBlock = {
  context: string
  bindings: Record<string, string>
}

/**
 * Filter out reserved shortcuts that cannot be rebound.
 * These would cause /doctor to warn, so we exclude them from the template.
 */
function filterReservedShortcuts(blocks: KeybindingBlock[]): KeybindingBlock[] {
  const reservedKeys = new Set(
    NON_REBINDABLE.map(r => normalizeKeyForComparison(r.key)),
  )

  return blocks
    .map(block => {
      const filteredBindings: Record<string, string | null> = {}
      for (const [key, action] of Object.entries(block.bindings)) {
        if (!reservedKeys.has(normalizeKeyForComparison(key))) {
          filteredBindings[key] = action
        }
      }
      return { context: block.context, bindings: filteredBindings }
    })
    .filter(block => Object.keys(block.bindings).length > 0)
}

/**
 * Generate a template keybindings.json file content.
 * Creates a fully valid JSON file with all default bindings that users can customize.
 */
export function generateKeybindingsTemplate(): string {
  // Filter out reserved shortcuts that cannot be rebound
  const bindings = filterReservedShortcuts(DEFAULT_BINDINGS)

  // Format as object wrapper with bindings array
  const config = {
    $schema: 'https://www.schemastore.org/claude-code-keybindings.json',
    $docs: 'https://docs.allternit.com/cli/keyboard-shortcuts',
    bindings,
  }

  return jsonStringify(config, null, 2) + '\n'
}
