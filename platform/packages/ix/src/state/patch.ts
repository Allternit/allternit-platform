/**
 * JSON Patch Engine (RFC 6902)
 * 
 * Implements JSON Patch operations for state synchronization.
 */

import type { StateStore } from './store';

export interface JSONPatchOperation {
  op: 'add' | 'remove' | 'replace' | 'move' | 'copy' | 'test';
  path: string;
  value?: unknown;
  from?: string;
}

export type JSONPatch = JSONPatchOperation[];

export interface PatchResult {
  success: boolean;
  operations: JSONPatchOperation[];
  failedAt?: number;
  error?: string;
  newState?: Record<string, unknown>;
}

/**
 * JSON Pointer helpers
 */
export function decodeJsonPointer(pointer: string): string[] {
  if (pointer === '' || pointer === '/') return [];
  if (!pointer.startsWith('/')) {
    throw new Error(`Invalid JSON Pointer: ${pointer}`);
  }
  
  return pointer
    .slice(1)
    .split('/')
    .map((segment) => segment.replace(/~1/g, '/').replace(/~0/g, '~'));
}

export function encodeJsonPointer(segments: string[]): string {
  if (segments.length === 0) return '';
  
  return '/' + segments
    .map((segment) => segment.replace(/~/g, '~0').replace(/\//g, '~1'))
    .join('/');
}

/**
 * Get value at JSON Pointer path
 */
export function getValueAtPath(obj: unknown, pointer: string): unknown {
  const segments = decodeJsonPointer(pointer);
  let current = obj;

  for (const segment of segments) {
    if (current === null || typeof current !== 'object') {
      return undefined;
    }

    if (Array.isArray(current)) {
      const index = segment === '-' ? current.length : parseInt(segment, 10);
      if (isNaN(index) || index < 0 || index > current.length) {
        return undefined;
      }
      current = current[index];
    } else {
      current = (current as Record<string, unknown>)[segment];
    }
  }

  return current;
}

/**
 * Set value at JSON Pointer path
 */
export function setValueAtPath(
  obj: unknown,
  pointer: string,
  value: unknown,
  operation: 'add' | 'replace' | 'remove'
): unknown {
  const segments = decodeJsonPointer(pointer);
  
  if (segments.length === 0) {
    return operation === 'remove' ? undefined : value;
  }

  // Clone to avoid mutation
  const root = deepClone(obj);
  let current: unknown = root;

  for (let i = 0; i < segments.length - 1; i++) {
    const segment = segments[i];
    const nextSegment = segments[i + 1];

    if (Array.isArray(current)) {
      const index = parseInt(segment, 10);
      if (isNaN(index) || index < 0 || index >= current.length) {
        throw new Error(`Array index out of bounds: ${segment}`);
      }
      
      // Ensure next level exists
      if (current[index] === null || typeof current[index] !== 'object') {
        const isNextArray = !isNaN(parseInt(nextSegment, 10)) || nextSegment === '-';
        current[index] = isNextArray ? [] : {};
      }
      current = current[index];
    } else {
      const currentObj = current as Record<string, unknown>;
      if (!(segment in currentObj) || currentObj[segment] === null || typeof currentObj[segment] !== 'object') {
        const isNextArray = !isNaN(parseInt(nextSegment, 10)) || nextSegment === '-';
        currentObj[segment] = isNextArray ? [] : {};
      }
      current = currentObj[segment];
    }
  }

  const lastSegment = segments[segments.length - 1];

  if (Array.isArray(current)) {
    const index = lastSegment === '-' ? current.length : parseInt(lastSegment, 10);
    
    if (operation === 'remove') {
      if (index < 0 || index >= current.length) {
        throw new Error(`Array index out of bounds: ${lastSegment}`);
      }
      current.splice(index, 1);
    } else if (operation === 'add' && lastSegment === '-') {
      current.push(value);
    } else {
      if (isNaN(index) || index < 0 || (operation === 'replace' && index >= current.length)) {
        throw new Error(`Invalid array index: ${lastSegment}`);
      }
      current[index] = value;
    }
  } else {
    const currentObj = current as Record<string, unknown>;
    
    if (operation === 'remove') {
      delete currentObj[lastSegment];
    } else {
      currentObj[lastSegment] = value;
    }
  }

  return root;
}

/**
 * Deep clone value
 */
function deepClone<T>(value: T): T {
  if (value === null || typeof value !== 'object') {
    return value;
  }

  if (Array.isArray(value)) {
    return value.map(deepClone) as unknown as T;
  }

  if (typeof value === 'object') {
    const cloned: Record<string, unknown> = {};
    for (const key of Object.keys(value)) {
      cloned[key] = deepClone((value as Record<string, unknown>)[key]);
    }
    return cloned as T;
  }

  return value;
}

/**
 * Deep equality check
 */
function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (a === null || b === null) return false;
  if (typeof a !== typeof b) return false;
  
  if (typeof a !== 'object') return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;

  if (Array.isArray(a)) {
    if (a.length !== (b as unknown[]).length) return false;
    for (let i = 0; i < a.length; i++) {
      if (!deepEqual(a[i], (b as unknown[])[i])) return false;
    }
    return true;
  }

  const keysA = Object.keys(a as object);
  const keysB = Object.keys(b as object);
  
  if (keysA.length !== keysB.length) return false;
  
  for (const key of keysA) {
    if (!keysB.includes(key)) return false;
    if (!deepEqual((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key])) {
      return false;
    }
  }

  return true;
}

/**
 * Apply single patch operation
 */
export function applyOperation(
  state: Record<string, unknown>,
  operation: JSONPatchOperation
): { success: boolean; newState: Record<string, unknown>; error?: string } {
  try {
    switch (operation.op) {
      case 'add': {
        if (operation.value === undefined) {
          return { success: false, newState: state, error: 'Add operation requires value' };
        }
        const newState = setValueAtPath(state, operation.path, operation.value, 'add');
        return { success: true, newState: newState as Record<string, unknown> };
      }

      case 'remove': {
        if (!getValueAtPath(state, operation.path)) {
          return { success: false, newState: state, error: `Path not found: ${operation.path}` };
        }
        const newState = setValueAtPath(state, operation.path, undefined, 'remove');
        return { success: true, newState: newState as Record<string, unknown> };
      }

      case 'replace': {
        if (operation.value === undefined) {
          return { success: false, newState: state, error: 'Replace operation requires value' };
        }
        if (getValueAtPath(state, operation.path) === undefined) {
          return { success: false, newState: state, error: `Path not found: ${operation.path}` };
        }
        const newState = setValueAtPath(state, operation.path, operation.value, 'replace');
        return { success: true, newState: newState as Record<string, unknown> };
      }

      case 'move': {
        if (!operation.from) {
          return { success: false, newState: state, error: 'Move operation requires from path' };
        }
        const value = getValueAtPath(state, operation.from);
        if (value === undefined) {
          return { success: false, newState: state, error: `Source path not found: ${operation.from}` };
        }
        let newState = setValueAtPath(state, operation.from, undefined, 'remove');
        newState = setValueAtPath(newState, operation.path, value, 'add');
        return { success: true, newState: newState as Record<string, unknown> };
      }

      case 'copy': {
        if (!operation.from) {
          return { success: false, newState: state, error: 'Copy operation requires from path' };
        }
        const value = getValueAtPath(state, operation.from);
        if (value === undefined) {
          return { success: false, newState: state, error: `Source path not found: ${operation.from}` };
        }
        const newState = setValueAtPath(state, operation.path, deepClone(value), 'add');
        return { success: true, newState: newState as Record<string, unknown> };
      }

      case 'test': {
        if (operation.value === undefined) {
          return { success: false, newState: state, error: 'Test operation requires value' };
        }
        const currentValue = getValueAtPath(state, operation.path);
        if (!deepEqual(currentValue, operation.value)) {
          return { 
            success: false, 
            newState: state, 
            error: `Test failed: expected ${JSON.stringify(operation.value)}, got ${JSON.stringify(currentValue)}` 
          };
        }
        return { success: true, newState: state };
      }

      default:
        return { success: false, newState: state, error: `Unknown operation: ${(operation as {op: string}).op}` };
    }
  } catch (error) {
    return { 
      success: false, 
      newState: state, 
      error: error instanceof Error ? error.message : String(error) 
    };
  }
}

/**
 * Apply JSON Patch to state
 */
export function applyPatch(
  state: Record<string, unknown>,
  patch: JSONPatch
): PatchResult {
  let currentState = deepClone(state);

  for (let i = 0; i < patch.length; i++) {
    const operation = patch[i];
    const result = applyOperation(currentState, operation);

    if (!result.success) {
      return {
        success: false,
        operations: patch.slice(0, i + 1),
        failedAt: i,
        error: result.error,
      };
    }

    currentState = result.newState;
  }

  return {
    success: true,
    operations: patch,
    newState: currentState,
  };
}

/**
 * Apply patch to state store
 */
export function applyPatchToStore(store: StateStore, patch: JSONPatch): PatchResult {
  const snapshot = store.snapshot();
  const result = applyPatch(snapshot, patch);

  if (result.success) {
    store.restore(result.operations.reduce((state, op) => {
      if (op.op !== 'test') {
        const { newState } = applyOperation(state, op);
        return newState;
      }
      return state;
    }, snapshot));
  }

  return result;
}

/**
 * Generate JSON Patch from two objects (diff)
 */
export function generatePatch(from: unknown, to: unknown, path = ''): JSONPatch {
  const patch: JSONPatch = [];

  // Handle primitive types
  if (typeof from !== typeof to || (typeof from !== 'object' && from !== to)) {
    if (from === undefined) {
      patch.push({ op: 'add', path, value: to });
    } else if (to === undefined) {
      patch.push({ op: 'remove', path });
    } else {
      patch.push({ op: 'replace', path, value: to });
    }
    return patch;
  }

  // Handle null
  if (from === null || to === null) {
    if (from !== to) {
      patch.push({ op: 'replace', path, value: to });
    }
    return patch;
  }

  // Handle arrays
  if (Array.isArray(from) && Array.isArray(to)) {
    // Simple array diff - could be optimized with LCS
    const maxLen = Math.max(from.length, to.length);
    for (let i = 0; i < maxLen; i++) {
      const itemPath = path + '/' + i;
      if (i >= from.length) {
        patch.push({ op: 'add', path: itemPath, value: to[i] });
      } else if (i >= to.length) {
        patch.push({ op: 'remove', path: itemPath });
      } else {
        patch.push(...generatePatch(from[i], to[i], itemPath));
      }
    }
    return patch;
  }

  // Handle objects
  if (typeof from === 'object' && typeof to === 'object') {
    const fromKeys = new Set(Object.keys(from as object));
    const toKeys = new Set(Object.keys(to as object));
    const allKeys = new Set([...fromKeys, ...toKeys]);

    for (const key of allKeys) {
      const keyPath = path ? `${path}/${key}` : `/${key}`;
      
      if (!fromKeys.has(key)) {
        // Added key
        patch.push({ op: 'add', path: keyPath, value: (to as Record<string, unknown>)[key] });
      } else if (!toKeys.has(key)) {
        // Removed key
        patch.push({ op: 'remove', path: keyPath });
      } else {
        // Modified key
        patch.push(...generatePatch(
          (from as Record<string, unknown>)[key],
          (to as Record<string, unknown>)[key],
          keyPath
        ));
      }
    }
    return patch;
  }

  return patch;
}

/**
 * Invert a patch for undo functionality
 */
export function invertPatch(patch: JSONPatch, state: Record<string, unknown>): JSONPatch {
  const inverted: JSONPatch = [];

  // Process in reverse order
  for (let i = patch.length - 1; i >= 0; i--) {
    const op = patch[i];

    switch (op.op) {
      case 'add': {
        // Undo add -> remove
        inverted.push({ op: 'remove', path: op.path });
        break;
      }

      case 'remove': {
        // Undo remove -> add with original value
        const originalValue = getValueAtPath(state, op.path);
        inverted.push({ op: 'add', path: op.path, value: originalValue });
        break;
      }

      case 'replace': {
        // Undo replace -> replace with original value
        const originalValue = getValueAtPath(state, op.path);
        inverted.push({ op: 'replace', path: op.path, value: originalValue });
        break;
      }

      case 'move': {
        // Undo move -> move back
        if (op.from) {
          inverted.push({ op: 'move', path: op.from, from: op.path });
        }
        break;
      }

      case 'copy': {
        // Undo copy -> remove
        inverted.push({ op: 'remove', path: op.path });
        break;
      }

      // test operations are self-inverting
      case 'test': {
        inverted.push(op);
        break;
      }
    }
  }

  return inverted;
}

/**
 * Validate patch operation
 */
export function validateOperation(op: JSONPatchOperation): { valid: boolean; error?: string } {
  if (!op.op || !['add', 'remove', 'replace', 'move', 'copy', 'test'].includes(op.op)) {
    return { valid: false, error: `Invalid operation: ${op.op}` };
  }

  if (!op.path || !op.path.startsWith('/')) {
    return { valid: false, error: `Invalid path: ${op.path}` };
  }

  if ((op.op === 'add' || op.op === 'replace' || op.op === 'test') && op.value === undefined) {
    return { valid: false, error: `${op.op} operation requires value` };
  }

  if ((op.op === 'move' || op.op === 'copy') && !op.from) {
    return { valid: false, error: `${op.op} operation requires from path` };
  }

  if ((op.op === 'move' || op.op === 'copy') && op.from && !op.from.startsWith('/')) {
    return { valid: false, error: `Invalid from path: ${op.from}` };
  }

  return { valid: true };
}

/**
 * Validate entire patch
 */
export function validatePatch(patch: JSONPatch): { valid: boolean; error?: string; index?: number } {
  for (let i = 0; i < patch.length; i++) {
    const result = validateOperation(patch[i]);
    if (!result.valid) {
      return { valid: false, error: result.error, index: i };
    }
  }
  return { valid: true };
}
