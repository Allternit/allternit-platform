/**
 * State Store
 * 
 * Scoped state management for Allternit-IX with binding support.
 */

import type { StateVariable, ComputedValue, Expression } from '../types';
import { evaluateExpression } from '../types';

export interface StateStore {
  /** Get value at path */
  get<T>(path: string): T | undefined;
  /** Set value at path */
  set<T>(path: string, value: T): void;
  /** Subscribe to changes */
  subscribe(path: string, callback: (value: unknown) => void): () => void;
  /** Compute derived value */
  compute<T>(name: string, deps: string[], compute: () => T): void;
  /** Bind component prop to state */
  bind(componentId: string, prop: string, statePath: string): void;
  /** Get all bindings for a component */
  getBindings(componentId: string): Array<{ prop: string; statePath: string }>;
  /** Batch multiple updates */
  batch(updates: Record<string, unknown>): void;
  /** Get state snapshot */
  snapshot(): Record<string, unknown>;
  /** Restore from snapshot */
  restore(snapshot: Record<string, unknown>): void;
  /** Reset to initial values */
  reset(): void;
}

export interface StateStoreConfig {
  /** Initial state values */
  initial?: Record<string, unknown>;
  /** State variable definitions */
  variables?: StateVariable[];
  /** Computed values */
  computed?: ComputedValue[];
  /** Scope ID (for persistence) */
  scopeId?: string;
  /** Persist to storage */
  persist?: boolean;
}

/**
 * Create state store
 */
export function createStateStore(config: StateStoreConfig = {}): StateStore {
  const state = new Map<string, unknown>();
  const subscribers = new Map<string, Set<(value: unknown) => void>>();
  const bindings = new Map<string, Map<string, string>>();
  const computedValues = new Map<string, { deps: string[]; compute: () => unknown }>();
  const computedCache = new Map<string, unknown>();
  const dirtyComputed = new Set<string>();

  // Initialize with initial values
  if (config.initial) {
    Object.entries(config.initial).forEach(([key, value]) => {
      state.set(key, value);
    });
  }

  // Initialize variables with defaults
  config.variables?.forEach((variable) => {
    if (!state.has(variable.path) && variable.default !== undefined) {
      state.set(variable.path, variable.default);
    }
  });

  // Notify subscribers
  function notify(path: string, value: unknown): void {
    const pathSubscribers = subscribers.get(path);
    if (pathSubscribers) {
      pathSubscribers.forEach((cb) => cb(value));
    }

    // Notify parent path subscribers
    const parts = path.split('.');
    while (parts.length > 1) {
      parts.pop();
      const parentPath = parts.join('.');
      const parentSubscribers = subscribers.get(parentPath);
      if (parentSubscribers) {
        const parentValue = get(parentPath);
        parentSubscribers.forEach((cb) => cb(parentValue));
      }
    }

    // Mark computed values as dirty
    computedValues.forEach((computed, name) => {
      if (computed.deps.some((dep) => path.startsWith(dep) || dep.startsWith(path))) {
        dirtyComputed.add(name);
      }
    });
  }

  // Get value at path
  function get<T>(path: string): T | undefined {
    // Check if it's a computed value
    if (computedValues.has(path)) {
      return getComputed<T>(path);
    }

    const parts = path.split('.');
    let current: unknown = Object.fromEntries(state);

    for (const part of parts) {
      if (current && typeof current === 'object') {
        current = (current as Record<string, unknown>)[part];
      } else {
        return undefined;
      }
    }

    return current as T;
  }

  // Get computed value
  function getComputed<T>(name: string): T | undefined {
    const computed = computedValues.get(name);
    if (!computed) return undefined;

    // Return cached value if not dirty
    if (!dirtyComputed.has(name) && computedCache.has(name)) {
      return computedCache.get(name) as T;
    }

    // Recompute
    const value = computed.compute();
    computedCache.set(name, value);
    dirtyComputed.delete(name);
    return value as T;
  }

  // Set value at path
  function set<T>(path: string, value: T): void {
    const parts = path.split('.');
    
    if (parts.length === 1) {
      state.set(path, value);
    } else {
      // Handle nested paths
      const rootKey = parts[0];
      let root = state.get(rootKey) as Record<string, unknown>;
      
      if (!root || typeof root !== 'object') {
        root = {};
        state.set(rootKey, root);
      }

      let current: Record<string, unknown> = root;
      for (let i = 1; i < parts.length - 1; i++) {
        const part = parts[i];
        if (!current[part] || typeof current[part] !== 'object') {
          current[part] = {};
        }
        current = current[part] as Record<string, unknown>;
      }

      current[parts[parts.length - 1]] = value;
      state.set(rootKey, { ...root });
    }

    notify(path, value);

    // Persist if enabled
    if (config.persist && config.scopeId) {
      persistState();
    }
  }

  // Subscribe to path changes
  function subscribe(path: string, callback: (value: unknown) => void): () => void {
    if (!subscribers.has(path)) {
      subscribers.set(path, new Set());
    }
    subscribers.get(path)!.add(callback);

    // Return unsubscribe function
    return () => {
      subscribers.get(path)?.delete(callback);
    };
  }

  // Register computed value
  function compute<T>(name: string, deps: string[], computeFn: () => T): void {
    computedValues.set(name, { deps, compute: computeFn });
    
    // Subscribe to dependencies
    deps.forEach((dep) => {
      subscribe(dep, () => {
        dirtyComputed.add(name);
        notify(name, getComputed(name));
      });
    });
  }

  // Bind component prop to state
  function bind(componentId: string, prop: string, statePath: string): void {
    if (!bindings.has(componentId)) {
      bindings.set(componentId, new Map());
    }
    bindings.get(componentId)!.set(prop, statePath);
  }

  // Get bindings for component
  function getBindings(componentId: string): Array<{ prop: string; statePath: string }> {
    const componentBindings = bindings.get(componentId);
    if (!componentBindings) return [];
    
    return Array.from(componentBindings.entries()).map(([prop, statePath]) => ({
      prop,
      statePath,
    }));
  }

  // Batch updates
  function batch(updates: Record<string, unknown>): void {
    Object.entries(updates).forEach(([path, value]) => {
      set(path, value);
    });
  }

  // Get snapshot
  function snapshot(): Record<string, unknown> {
    return Object.fromEntries(state);
  }

  // Restore from snapshot
  function restore(data: Record<string, unknown>): void {
    Object.entries(data).forEach(([key, value]) => {
      state.set(key, value);
      notify(key, value);
    });
  }

  // Reset to initial
  function reset(): void {
    state.clear();
    computedCache.clear();
    dirtyComputed.clear();

    if (config.initial) {
      Object.entries(config.initial).forEach(([key, value]) => {
        state.set(key, value);
      });
    }

    // Notify all subscribers
    subscribers.forEach((cbs, path) => {
      const value = get(path);
      cbs.forEach((cb) => cb(value));
    });
  }

  // Persist to localStorage
  function persistState(): void {
    if (typeof localStorage === 'undefined') return;
    
    try {
      const key = `allternit-ix-state-${config.scopeId}`;
      localStorage.setItem(key, JSON.stringify(Object.fromEntries(state)));
    } catch (error) {
      console.error('Failed to persist state:', error);
    }
  }

  // Load from localStorage
  function loadPersistedState(): void {
    if (typeof localStorage === 'undefined' || !config.scopeId) return;
    
    try {
      const key = `allternit-ix-state-${config.scopeId}`;
      const data = localStorage.getItem(key);
      if (data) {
        const parsed = JSON.parse(data);
        Object.entries(parsed).forEach(([key, value]) => {
          state.set(key, value);
        });
      }
    } catch (error) {
      console.error('Failed to load persisted state:', error);
    }
  }

  // Load persisted state on init
  if (config.persist && config.scopeId) {
    loadPersistedState();
  }

  return {
    get,
    set,
    subscribe,
    compute,
    bind,
    getBindings,
    batch,
    snapshot,
    restore,
    reset,
  };
}

// Global stores
const stores = new Map<string, StateStore>();

export function getStateStore(scopeId: string, config?: StateStoreConfig): StateStore {
  if (!stores.has(scopeId)) {
    stores.set(scopeId, createStateStore({ ...config, scopeId }));
  }
  return stores.get(scopeId)!;
}

export function clearStateStore(scopeId: string): void {
  stores.delete(scopeId);
}
