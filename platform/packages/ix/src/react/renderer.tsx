/**
 * React Renderer for Allternit-IX
 * 
 * Renders Allternit-IX UI IR to React components.
 */

import React, { useState, useEffect, useCallback, useMemo } from 'react';
import type { UIRoot, UIComponent, UIAction, Expression, StateBinding } from '../types';
import type { StateStore } from '../state/store';
import { createStateStore } from '../state/store';
import type { ComponentCatalog } from '../catalog/registry';
import { createDefaultCatalog } from '../catalog/registry';
import type { JSONPatch } from '../state/patch';
import { applyPatchToStore } from '../state/patch';
import { evaluateExpression } from '../types';

export interface ReactRendererConfig {
  /** Component catalog */
  catalog?: ComponentCatalog;
  /** Initial state store */
  stateStore?: StateStore;
  /** Component mapping (type -> React component) */
  components?: Record<string, React.ComponentType<unknown>>;
  /** Event handler */
  onEvent?: (name: string, payload: unknown) => void;
  /** Error handler */
  onError?: (error: Error, componentId?: string) => void;
  /** Debug mode */
  debug?: boolean;
}

export interface ReactRenderer {
  /** Render UI root */
  UIRoot: React.FC<{ root: UIRoot }>;
  /** Render single component */
  UIComponent: React.FC<{ component: UIComponent }>;
  /** Get state value */
  getState: (path: string) => unknown;
  /** Set state value */
  setState: (path: string, value: unknown) => void;
  /** Apply patch */
  applyPatch: (patch: JSONPatch) => void;
  /** Dispatch action */
  dispatch: (actionId: string, params?: Record<string, unknown>) => void;
}

/**
 * Create React renderer
 */
export function createReactRenderer(config: ReactRendererConfig): ReactRenderer {
  const catalog = config.catalog ?? createDefaultCatalog();
  const customComponents = config.components ?? {};
  
  // Context for state store
  const StoreContext = React.createContext<StateStore | null>(null);

  /**
   * Hook to use state from store
   */
  function useStoreState<T>(path: string): [T | undefined, (value: T) => void] {
    const store = React.useContext(StoreContext);
    if (!store) throw new Error('Store not available');

    const [value, setValue] = useState<T | undefined>(() => store.get<T>(path));

    useEffect(() => {
      const unsubscribe = store.subscribe(path, (newValue) => {
        setValue(newValue as T);
      });
      return unsubscribe;
    }, [store, path]);

    const updateValue = useCallback((newValue: T) => {
      store.set(path, newValue);
    }, [store, path]);

    return [value, updateValue];
  }

  /**
   * Hook to evaluate expression
   */
  function useExpression(expr: Expression | undefined, localContext: Record<string, unknown> = {}) {
    const store = React.useContext(StoreContext);
    
    return useMemo(() => {
      if (expr === undefined) return undefined;
      const context = { ...store?.snapshot(), ...localContext };
      return evaluateExpression(expr, context);
    }, [expr, store, localContext]);
  }

  /**
   * Component renderer
   */
  const UIComponentRenderer: React.FC<{ 
    component: UIComponent; 
    localContext?: Record<string, unknown>;
  }> = ({ component, localContext = {} }) => {
    const store = React.useContext(StoreContext);
    const [error, setError] = useState<Error | null>(null);

    // Handle errors
    if (error) {
      config.onError?.(error, component.id);
      return <ErrorFallback error={error} componentId={component.id} />;
    }

    try {
      // Evaluate condition
      if (component.condition) {
        const conditionValue = evaluateExpression(component.condition, {
          ...store?.snapshot(),
          ...localContext,
        });
        if (!conditionValue) return null;
      }

      // Handle repeat
      if (component.repeat) {
        return <RepeatComponent component={component} localContext={localContext} />;
      }

      return <SingleComponent component={component} localContext={localContext} />;
    } catch (err) {
      setError(err as Error);
      return null;
    }
  };

  /**
   * Repeat component renderer
   */
  const RepeatComponent: React.FC<{
    component: UIComponent;
    localContext: Record<string, unknown>;
  }> = ({ component, localContext }) => {
    const store = React.useContext(StoreContext);
    const repeat = component.repeat!;
    
    const items = store?.get<unknown[]>(repeat.items) ?? [];
    
    // Apply filter if specified
    let filteredItems = items;
    if (repeat.filter) {
      filteredItems = items.filter((item, index) => {
        const ctx = { ...localContext, [repeat.as]: item, ...(repeat.indexAs ? { [repeat.indexAs]: index } : {}) };
        return evaluateExpression(repeat.filter!, { ...store?.snapshot(), ...ctx });
      });
    }

    // Apply sort if specified
    if (repeat.sort) {
      filteredItems = [...filteredItems].sort((a, b) => {
        const ctxA = { ...localContext, [repeat.as]: a };
        const ctxB = { ...localContext, [repeat.as]: b };
        const valA = evaluateExpression(repeat.sort!, { ...store?.snapshot(), ...ctxA }) as number | string;
        const valB = evaluateExpression(repeat.sort!, { ...store?.snapshot(), ...ctxB }) as number | string;
        if (valA < valB) return -1;
        if (valA > valB) return 1;
        return 0;
      });
    }

    return (
      <>
        {filteredItems.map((item, index) => {
          const itemContext = {
            ...localContext,
            [repeat.as]: item,
            ...(repeat.indexAs ? { [repeat.indexAs]: index } : {}),
          };
          return (
            <SingleComponent
              key={`${component.id}-${index}`}
              component={{ ...component, id: `${component.id}-${index}` }}
              localContext={itemContext}
            />
          );
        })}
      </>
    );
  };

  /**
   * Single component renderer
   */
  const SingleComponent: React.FC<{
    component: UIComponent;
    localContext: Record<string, unknown>;
  }> = ({ component, localContext }) => {
    const store = React.useContext(StoreContext);
    const [props, setProps] = useState<Record<string, unknown>>({});

    // Build props with bindings
    useEffect(() => {
      const newProps = { ...component.props };

      // Apply bindings
      component.bindings?.forEach((binding) => {
        const value = store?.get(binding.statePath);
        newProps[binding.prop] = value;
      });

      setProps(newProps);
    }, [component, store]);

    // Subscribe to binding changes
    useEffect(() => {
      if (!component.bindings || !store) return;

      const unsubscribes = component.bindings.map((binding) =>
        store.subscribe(binding.statePath, (value) => {
          setProps((prev) => ({ ...prev, [binding.prop]: value }));
        })
      );

      return () => unsubscribes.forEach((unsub) => unsub());
    }, [component.bindings, store]);

    // Handle events
    const handleEvent = useCallback((eventName: string, actionId: string, params?: Record<string, unknown>) => {
      return (...eventArgs: unknown[]) => {
        const fullContext = {
          ...store?.snapshot(),
          ...localContext,
          event: eventArgs[0],
        };
        dispatchAction(actionId, params, fullContext);
      };
    }, [store, localContext]);

    // Get component type
    const Component = customComponents[component.type] || getBuiltinComponent(component.type);

    if (!Component) {
      config.onError?.(new Error(`Unknown component type: ${component.type}`), component.id);
      return null;
    }

    // Build final props
    const finalProps: Record<string, unknown> = { ...props };

    // Add event handlers
    component.events?.forEach((event) => {
      finalProps[event.event] = handleEvent(event.event, event.action, event.params);
    });

    // Add two-way binding handlers
    component.bindings?.forEach((binding) => {
      if (binding.direction === 'two-way') {
        const changeHandlerName = `on${capitalize(binding.prop)}Change`;
        finalProps[changeHandlerName] = (value: unknown) => {
          store?.set(binding.statePath, value);
        };
      }
    });

    // Render children
    const children = component.children?.map((child) => (
      <UIComponentRenderer key={child.id} component={child} localContext={localContext} />
    ));

    // Use createElement for dynamic component rendering
    return React.createElement(Component, { ...finalProps, key: component.id }, children?.length ? children : null);
  };

  /**
   * Error fallback component
   */
  const ErrorFallback: React.FC<{ error: Error; componentId?: string }> = ({ error, componentId }) => {
    if (process.env.NODE_ENV === 'production') {
      return null;
    }
    return (
      <div style={{ 
        padding: '12px', 
        background: '#fef2f2', 
        border: '1px solid #fecaca',
        borderRadius: '4px',
        color: '#dc2626',
        fontSize: '12px',
      }}>
        <strong>Component Error{componentId ? ` (${componentId})` : ''}</strong>
        <pre style={{ margin: '8px 0 0', whiteSpace: 'pre-wrap' }}>{error.message}</pre>
      </div>
    );
  };

  /**
   * Root component
   */
  const UIRootComponent: React.FC<{ root: UIRoot }> = ({ root }) => {
    const [store, setStore] = useState<StateStore | null>(null);

    useEffect(() => {
      const newStore = config.stateStore ?? createStateStore({
        initial: root.state.initial,
        variables: root.state.variables,
      });

      // Initialize state
      if (root.state.initial) {
        Object.entries(root.state.initial).forEach(([path, value]) => {
          newStore.set(path, value);
        });
      }

      setStore(newStore);
    }, [root]);

    if (!store) return null;

    return (
      <StoreContext.Provider value={store}>
        <div className="allternit-ix-root" data-version={root.version}>
          {root.components.map((component) => (
            <UIComponentRenderer key={component.id} component={component} />
          ))}
        </div>
        {root.css && <style>{root.css}</style>}
      </StoreContext.Provider>
    );
  };

  /**
   * Get builtin component
   */
  function getBuiltinComponent(type: string): React.ComponentType<unknown> | undefined {
    const builtins: Record<string, React.ComponentType<unknown>> = {
      Box: (({ children, ...props }: { children?: React.ReactNode; [key: string]: unknown }) => 
        <div style={convertBoxProps(props)}>{children}</div>) as React.ComponentType<unknown>,
      Stack: (({ children, direction = 'vertical', spacing = 0, ...props }: 
        { children?: React.ReactNode; direction?: string; spacing?: number; [key: string]: unknown }) => (
        <div style={{
          display: 'flex',
          flexDirection: direction === 'horizontal' ? 'row' : 'column',
          gap: `${spacing * 4}px`,
          ...convertBoxProps(props),
        }}>
          {children}
        </div>
      )) as React.ComponentType<unknown>,
      Text: (({ children, size = 'md', weight = 'normal', color, align, ...props }: 
        { children?: React.ReactNode; size?: string; weight?: string; color?: string; align?: string; [key: string]: unknown }) => (
        <span style={{
          fontSize: getTextSize(size),
          fontWeight: getFontWeight(weight),
          color,
          textAlign: align as 'left' | 'center' | 'right',
          ...props.style as object,
        }}>
          {children}
        </span>
      )) as React.ComponentType<unknown>,
      Heading: (({ children, level = 1, color, ...props }: 
        { children?: React.ReactNode; level?: number; color?: string; [key: string]: unknown }) => {
        const Tag = `h${level}` as keyof JSX.IntrinsicElements;
        return (
          <Tag style={{
            fontSize: getHeadingSize(level),
            fontWeight: 'bold',
            color,
            margin: '0 0 16px',
            ...props.style as object,
          }}>
            {children}
          </Tag>
        );
      }) as React.ComponentType<unknown>,
      Paragraph: ({ children, size = 'md', color, ...props }) => (
        <p style={{
          fontSize: getTextSize(size),
          color,
          margin: '0 0 12px',
          lineHeight: 1.6,
          ...props.style as object,
        }}>
          {children}
        </p>
      ),
      Button: ({ children, variant = 'primary', size = 'md', disabled, loading, onClick, ...props }) => (
        <button
          onClick={onClick as React.MouseEventHandler}
          disabled={disabled || loading}
          style={{
            padding: size === 'sm' ? '6px 12px' : size === 'lg' ? '12px 24px' : '8px 16px',
            fontSize: size === 'sm' ? '14px' : size === 'lg' ? '18px' : '16px',
            background: getButtonColor(variant),
            color: '#fff',
            border: 'none',
            borderRadius: '6px',
            cursor: disabled ? 'not-allowed' : 'pointer',
            opacity: disabled ? 0.6 : 1,
            ...props.style as object,
          }}
        >
          {loading ? 'Loading...' : children}
        </button>
      ),
      Input: ({ value, placeholder, type = 'text', disabled, required, onChange, onBlur, onFocus, ...props }) => (
        <input
          type={type}
          value={value as string}
          placeholder={placeholder}
          disabled={disabled}
          required={required}
          onChange={(e) => onChange?.(e.target.value)}
          onBlur={onBlur}
          onFocus={onFocus}
          style={{
            padding: '8px 12px',
            fontSize: '16px',
            border: '1px solid #d1d5db',
            borderRadius: '6px',
            width: '100%',
            ...props.style as object,
          }}
        />
      ),
      TextArea: ({ value, placeholder, rows = 3, disabled, onChange, ...props }) => (
        <textarea
          value={value as string}
          placeholder={placeholder}
          rows={rows}
          disabled={disabled}
          onChange={(e) => onChange?.(e.target.value)}
          style={{
            padding: '8px 12px',
            fontSize: '16px',
            border: '1px solid #d1d5db',
            borderRadius: '6px',
            width: '100%',
            resize: 'vertical',
            ...props.style as object,
          }}
        />
      ),
      Select: ({ value, options, placeholder, disabled, onChange, ...props }) => (
        <select
          value={value as string}
          disabled={disabled}
          onChange={(e) => onChange?.(e.target.value)}
          style={{
            padding: '8px 12px',
            fontSize: '16px',
            border: '1px solid #d1d5db',
            borderRadius: '6px',
            width: '100%',
            ...props.style as object,
          }}
        >
          {placeholder && <option value="">{placeholder}</option>}
          {(options as Array<{ value: string; label: string }>)?.map((opt) => (
            <option key={opt.value} value={opt.value}>{opt.label}</option>
          ))}
        </select>
      ),
      Card: ({ children, padding = 16, elevation = 1, borderRadius = 8, ...props }) => (
        <div style={{
          padding: `${padding}px`,
          background: '#fff',
          borderRadius: `${borderRadius}px`,
          boxShadow: getElevation(elevation),
          ...props.style as object,
        }}>
          {children}
        </div>
      ),
      Badge: ({ children, variant = 'default', size = 'md' }) => (
        <span style={{
          display: 'inline-flex',
          alignItems: 'center',
          padding: size === 'sm' ? '2px 6px' : '4px 10px',
          fontSize: size === 'sm' ? '12px' : '14px',
          fontWeight: 500,
          background: getBadgeColor(variant),
          color: '#fff',
          borderRadius: '9999px',
        }}>
          {children}
        </span>
      ),
      Image: ({ src, alt = '', width, height, objectFit, ...props }) => (
        <img
          src={src as string}
          alt={alt}
          width={width}
          height={height}
          style={{
            objectFit: objectFit as 'cover' | 'contain' | 'fill' | 'none',
            maxWidth: '100%',
            ...props.style as object,
          }}
        />
      ),
      Icon: ({ name, size = 16, color }) => (
        <span style={{
          width: `${size}px`,
          height: `${size}px`,
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'center',
          color,
        }}>
          {/* Simple text icon fallback */}
          {name}
        </span>
      ),
      List: ({ items, renderItem, emptyText = 'No items' }) => {
        if (!items || (items as unknown[]).length === 0) {
          return <div style={{ color: '#6b7280', fontStyle: 'italic' }}>{emptyText}</div>;
        }
        return (
          <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
            {(items as unknown[]).map((item, index) => (
              <li key={index}>{(renderItem as Function)(item, index)}</li>
            ))}
          </ul>
        );
      },
      Table: ({ data, columns, onRowClick, emptyText = 'No data' }) => {
        if (!data || (data as unknown[]).length === 0) {
          return <div style={{ color: '#6b7280', fontStyle: 'italic' }}>{emptyText}</div>;
        }
        return (
          <table style={{ width: '100%', borderCollapse: 'collapse' }}>
            <thead>
              <tr>
                {(columns as Array<{ key: string; title: string }>).map((col) => (
                  <th key={col.key} style={{ textAlign: 'left', padding: '8px', borderBottom: '1px solid #e5e7eb' }}>
                    {col.title}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {(data as unknown[]).map((row, index) => (
                <tr
                  key={index}
                  onClick={() => onRowClick?.(row, index)}
                  style={{ cursor: onRowClick ? 'pointer' : undefined }}
                >
                  {(columns as Array<{ key: string; render?: Function }>).map((col) => (
                    <td key={col.key} style={{ padding: '8px', borderBottom: '1px solid #e5e7eb' }}>
                      {col.render ? col.render((row as Record<string, unknown>)[col.key], row) : (row as Record<string, unknown>)[col.key]}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        );
      },
    };

    return builtins[type];
  }

  /**
   * Dispatch action
   */
  function dispatchAction(
    actionId: string,
    params: Record<string, unknown> | undefined,
    context: Record<string, unknown>
  ): void {
    config.onEvent?.(actionId, { params, context });
  }

  // Public API
  return {
    UIRoot: UIRootComponent,
    UIComponent: UIComponentRenderer,
    getState: (path: string) => config.stateStore?.get(path),
    setState: (path: string, value: unknown) => config.stateStore?.set(path, value),
    applyPatch: (patch: JSONPatch) => {
      if (config.stateStore) {
        applyPatchToStore(config.stateStore, patch);
      }
    },
    dispatch: (actionId: string, params?: Record<string, unknown>) => {
      const context = config.stateStore?.snapshot() ?? {};
      dispatchAction(actionId, params, context);
    },
  };
}

// Helper functions
function capitalize(str: string): string {
  return str.charAt(0).toUpperCase() + str.slice(1);
}

function convertBoxProps(props: Record<string, unknown>): React.CSSProperties {
  return {
    padding: props.padding !== undefined ? `${(props.padding as number) * 4}px` : undefined,
    margin: props.margin !== undefined ? `${(props.margin as number) * 4}px` : undefined,
    width: props.width as string | number,
    height: props.height as string | number,
    display: props.display as 'block' | 'flex' | 'grid' | 'none',
    flexDirection: props.flexDirection as 'row' | 'column',
    justifyContent: props.justifyContent as string,
    alignItems: props.alignItems as string,
    gap: props.gap !== undefined ? `${(props.gap as number) * 4}px` : undefined,
    backgroundColor: props.backgroundColor as string,
    borderRadius: props.borderRadius !== undefined ? `${props.borderRadius}px` : undefined,
  };
}

function getTextSize(size: string): string {
  const sizes: Record<string, string> = { xs: '12px', sm: '14px', md: '16px', lg: '18px', xl: '20px' };
  return sizes[size] || sizes.md;
}

function getHeadingSize(level: number): string {
  const sizes: Record<number, string> = { 1: '32px', 2: '24px', 3: '20px', 4: '18px', 5: '16px', 6: '14px' };
  return sizes[level] || sizes[1];
}

function getFontWeight(weight: string): number {
  const weights: Record<string, number> = { normal: 400, medium: 500, semibold: 600, bold: 700 };
  return weights[weight] || weights.normal;
}

function getButtonColor(variant: string): string {
  const colors: Record<string, string> = {
    primary: '#3b82f6',
    secondary: '#6b7280',
    ghost: 'transparent',
    danger: '#ef4444',
  };
  return colors[variant] || colors.primary;
}

function getBadgeColor(variant: string): string {
  const colors: Record<string, string> = {
    default: '#6b7280',
    success: '#22c55e',
    warning: '#f59e0b',
    error: '#ef4444',
    info: '#3b82f6',
  };
  return colors[variant] || colors.default;
}

function getElevation(level: number): string {
  const shadows: Record<number, string> = {
    0: 'none',
    1: '0 1px 3px rgba(0,0,0,0.1)',
    2: '0 4px 6px rgba(0,0,0,0.1)',
    3: '0 10px 15px rgba(0,0,0,0.1)',
  };
  return shadows[level] || shadows[1];
}
