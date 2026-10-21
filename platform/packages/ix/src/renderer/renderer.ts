/**
 * UI Renderer
 * 
 * Core renderer for Allternit-IX UI IR. Platform-agnostic base that delegates
to platform-specific renderers (React, Vue, etc.).
 */

import type { UIRoot, UIComponent, UIAction, UIState } from '../types';
import type { StateStore } from '../state/store';
import type { ComponentCatalog } from '../catalog/registry';
import type { JSONPatch } from '../state/patch';

export interface UIRendererConfig {
  /** Component catalog */
  catalog?: ComponentCatalog;
  /** Platform renderer */
  platform: PlatformRenderer;
  /** State store instance */
  stateStore?: StateStore;
  /** Event handler */
  onEvent?: (name: string, payload: unknown) => void;
  /** Error handler */
  onError?: (error: Error, componentId?: string) => void;
  /** Debug mode */
  debug?: boolean;
}

export interface PlatformRenderer {
  /** Create element */
  createElement(
    type: string,
    props: Record<string, unknown>,
    children: unknown[]
  ): unknown;
  /** Create text node */
  createTextNode(text: string): unknown;
  /** Render to container */
  render(element: unknown, container: unknown): void;
  /** Unmount */
  unmount(container: unknown): void;
}

export interface UIRenderer {
  /** Render UI to container */
  render(root: UIRoot, container: unknown): void;
  /** Update UI with patch */
  update(patch: JSONPatch): void;
  /** Dispatch action */
  dispatch(actionId: string, params?: Record<string, unknown>): void;
  /** Get state */
  getState(path?: string): unknown;
  /** Set state */
  setState(path: string, value: unknown): void;
  /** Subscribe to state changes */
  subscribe(path: string, callback: (value: unknown) => void): () => void;
  /** Destroy renderer */
  destroy(): void;
}

/**
 * Create UI renderer
 */
export function createUIRenderer(config: UIRendererConfig): UIRenderer {
  let currentRoot: UIRoot | null = null;
  let currentContainer: unknown = null;
  let destroyed = false;

  const stateStore = config.stateStore;
  const catalog = config.catalog;
  const platform = config.platform;

  function render(root: UIRoot, container: unknown): void {
    if (destroyed) {
      throw new Error('Renderer has been destroyed');
    }

    currentRoot = root;
    currentContainer = container;

    // Initialize state
    if (stateStore && root.state?.initial) {
      Object.entries(root.state.initial).forEach(([path, value]) => {
        stateStore.set(path, value);
      });
    }

    // Render component tree
    const elements = root.components.map((comp) => renderComponent(comp));
    const rootElement = platform.createElement('Fragment', {}, elements);
    
    platform.render(rootElement, container);
  }

  function renderComponent(component: UIComponent, context?: Record<string, unknown>): unknown {
    try {
      // Validate component against catalog
      if (catalog && !catalog.validate(component.type, component.props)) {
        throw new Error(`Invalid props for component type: ${component.type}`);
      }

      // Build props with context evaluation
      const props: Record<string, unknown> = {};
      if (component.props) {
        Object.entries(component.props).forEach(([key, value]) => {
          props[key] = evaluateValue(value, context || {});
        });
      }

      // Add bindings
      if (component.bindings && stateStore) {
        component.bindings.forEach((binding) => {
          const value = stateStore.get(binding.statePath);
          props[binding.prop] = value;

          if (binding.direction === 'two-way') {
            // Add onChange handler
            props[`on${capitalize(binding.prop)}Change`] = (newValue: unknown) => {
              stateStore.set(binding.statePath, newValue);
            };
          }
        });
      }

      // Add event handlers
      if (component.events) {
        component.events.forEach((event) => {
          props[event.event] = (...args: unknown[]) => {
            handleEvent(event.action, event.params, args);
          };
        });
      }

      // Render children
      let children: unknown[] = [];
      if (component.children) {
        children = component.children.map(child => renderComponent(child, context));
      }

      // Handle repeat
      if (component.repeat && stateStore) {
        const items = stateStore.get<unknown[]>(component.repeat.items) || [];
        children = items.map((item, index) => {
          const itemContext = {
            ...context,
            [component.repeat!.as]: item,
            ...(component.repeat!.indexAs ? { [component.repeat!.indexAs]: index } : {}),
          };
          return renderComponent(
            component.children?.[0] || { id: generateId(), type: 'Fragment', props: {} },
            itemContext
          );
        });
      }

      // Add key/id
      props.key = component.id;

      // Create element
      return platform.createElement(component.type, props, children);
    } catch (error) {
      config.onError?.(error as Error, component.id);
      return platform.createElement('ErrorBoundary', { error }, []);
    }
  }

  function handleEvent(
    actionId: string,
    params: Record<string, unknown> | undefined,
    eventArgs: unknown[]
  ): void {
    if (!currentRoot) return;

    const action = currentRoot.actions.find((a) => a.id === actionId);
    if (!action) {
      config.onError?.(new Error(`Action not found: ${actionId}`));
      return;
    }

    executeAction(action, params, eventArgs);
  }

  function executeAction(
    action: UIAction,
    params: Record<string, unknown> | undefined,
    eventArgs: unknown[]
  ): void {
    const { handler } = action;

    switch (handler.type) {
      case 'setState': {
        if (stateStore) {
          // Evaluate value expression with params and event
          const context = { params, event: eventArgs[0], ...stateStore.snapshot() };
          const value = evaluateValue(handler.value, context);
          stateStore.set(handler.path, value);
        }
        break;
      }

      case 'emitEvent': {
        const context = { params, event: eventArgs[0], ...stateStore?.snapshot() };
        const payload = evaluateValue(handler.payload, context);
        config.onEvent?.(handler.name, payload);
        break;
      }

      case 'navigate': {
        // Navigation handled by platform
        break;
      }

      case 'validate': {
        // Validation logic
        break;
      }

      case 'custom': {
        config.onEvent?.(action.id, { params, event: eventArgs[0] });
        break;
      }
    }
  }

  function update(patch: JSONPatch): void {
    if (!stateStore || destroyed) return;

    // Apply patch to state
    const snapshot = stateStore.snapshot();
    
    patch.forEach((op) => {
      if (op.op === 'add' || op.op === 'replace') {
        const path = op.path.slice(1).replace(/\//g, '.');
        stateStore.set(path, op.value);
      } else if (op.op === 'remove') {
        const path = op.path.slice(1).replace(/\//g, '.');
        stateStore.set(path, undefined);
      }
    });

    // Re-render if needed
    if (currentRoot && currentContainer) {
      render(currentRoot, currentContainer);
    }
  }

  function dispatch(actionId: string, params?: Record<string, unknown>): void {
    handleEvent(actionId, params, []);
  }

  function getState(path?: string): unknown {
    if (!stateStore) return undefined;
    return path ? stateStore.get(path) : stateStore.snapshot();
  }

  function setState(path: string, value: unknown): void {
    stateStore?.set(path, value);
  }

  function subscribe(path: string, callback: (value: unknown) => void): () => void {
    if (!stateStore) return () => {};
    return stateStore.subscribe(path, callback);
  }

  function destroy(): void {
    if (currentContainer) {
      platform.unmount(currentContainer);
    }
    destroyed = true;
    currentRoot = null;
    currentContainer = null;
  }

  return {
    render,
    update,
    dispatch,
    getState,
    setState,
    subscribe,
    destroy,
  };
}

/**
 * Evaluate value expression
 */
function evaluateValue(value: unknown, context: Record<string, unknown>): unknown {
  if (value === null || typeof value !== 'object') {
    return value;
  }

  if ('$path' in (value as object)) {
    const path = (value as { $path: string }).$path;
    return path.split('.').reduce((acc: unknown, key) => {
      if (acc && typeof acc === 'object') {
        return (acc as Record<string, unknown>)[key];
      }
      return undefined;
    }, context);
  }

  if ('$expr' in (value as object)) {
    const expr = (value as { $expr: string }).$expr;
    try {
      // Simple expression evaluation
      const fn = new Function('context', `with(context) { return ${expr}; }`);
      return fn(context);
    } catch {
      return undefined;
    }
  }

  return value;
}

/**
 * Generate unique ID
 */
function generateId(): string {
  return `comp_${Math.random().toString(36).substr(2, 9)}`;
}

/**
 * Capitalize string
 */
function capitalize(str: string): string {
  return str.charAt(0).toUpperCase() + str.slice(1);
}
