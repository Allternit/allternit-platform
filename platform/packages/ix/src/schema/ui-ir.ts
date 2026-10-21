/**
 * UI Intermediate Representation Schema
 * 
 * Core schema definitions for declarative UI generation.
 */

import type { 
  UIRoot, 
  UIComponent, 
  UIState, 
  UIAction,
  StateVariable,
  StateBinding,
  EventHandler,
  Expression,
} from '../types';

export type {
  UIRoot,
  UIComponent,
  UIState,
  UIAction,
  StateVariable,
  StateBinding,
  EventHandler,
  Expression,
};

/**
 * Schema version
 */
export const UI_IR_VERSION = '1.0.0';

/**
 * Validate UIRoot structure
 */
export function validateUIRoot(root: unknown): root is UIRoot {
  if (!root || typeof root !== 'object') return false;
  
  const r = root as Partial<UIRoot>;
  
  if (typeof r.version !== 'string') return false;
  if (!Array.isArray(r.components)) return false;
  if (!r.state || typeof r.state !== 'object') return false;
  if (!Array.isArray(r.actions)) return false;
  
  return true;
}

/**
 * Create empty UI root
 */
export function createEmptyRoot(): UIRoot {
  return {
    version: UI_IR_VERSION,
    components: [],
    state: {
      variables: [],
    },
    actions: [],
  };
}

/**
 * Create UI root from template
 */
export function createRootFromTemplate(
  template: Partial<UIRoot> & { components: UIComponent[] }
): UIRoot {
  return {
    version: UI_IR_VERSION,
    state: { variables: [] },
    actions: [],
    ...template,
  };
}
