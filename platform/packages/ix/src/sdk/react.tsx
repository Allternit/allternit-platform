/**
 * Allternit-IX React SDK
 * 
 * React hooks and components for Allternit-IX integration.
 */

import React, { useState, useEffect, useCallback, useRef, createContext, useContext } from 'react';
import type { UIRoot } from '../types';
import type { JSONPatch } from '../state/patch';
import { createReactRenderer } from '../react/renderer';
import { IXCapsuleClient, type IXCapsuleClientConfig, type IXCapsuleEvent } from './api';

// Context for IX capsule state
interface IXContextValue {
  client: IXCapsuleClient | null;
  capsuleId: string | null;
  state: Record<string, unknown>;
  dispatch: (actionId: string, params?: Record<string, unknown>) => void;
  applyPatch: (patch: JSONPatch) => void;
}

const IXContext = createContext<IXContextValue | null>(null);

/**
 * Hook to use IX context
 */
export function useIX(): IXContextValue {
  const context = useContext(IXContext);
  if (!context) {
    throw new Error('useIX must be used within IXProvider');
  }
  return context;
}

/**
 * Props for IXProvider
 */
interface IXProviderProps {
  /** API configuration */
  config: IXCapsuleClientConfig;
  /** Capsule ID (if already created) */
  capsuleId?: string;
  /** UI definition (for creating new capsule) */
  ui?: UIRoot;
  /** Children */
  children: React.ReactNode;
}

/**
 * IX Provider component
 */
export function IXProvider({ config, capsuleId: initialCapsuleId, ui, children }: IXProviderProps): React.ReactElement {
  const [client] = useState(() => createIXClient(config));
  const [capsuleId, setCapsuleId] = useState<string | null>(initialCapsuleId || null);
  const [state, setState] = useState<Record<string, unknown>>({});
  const eventHandlerRef = useRef<(event: IXCapsuleEvent) => void>();

  // Create capsule if UI provided and no capsuleId
  useEffect(() => {
    if (!capsuleId && ui) {
      client.createCapsule({ ui }).then((response) => {
        setCapsuleId(response.capsule_id);
      });
    }
  }, [client, capsuleId, ui]);

  // Set up event handler
  useEffect(() => {
    eventHandlerRef.current = (event: IXCapsuleEvent) => {
      if (event.event_type === 'state-change' && event.state_update) {
        setState((prev) => ({ ...prev, ...event.state_update }));
      }
    };
  }, []);

  // Update config onEvent handler
  useEffect(() => {
    const originalOnEvent = config.onEvent;
    config.onEvent = (event: IXCapsuleEvent) => {
      eventHandlerRef.current?.(event);
      originalOnEvent?.(event);
    };
  }, [config]);

  // Fetch initial state
  useEffect(() => {
    if (capsuleId) {
      client.getCapsule(capsuleId).then((capsule) => {
        setState(capsule.state);
      });
    }
  }, [client, capsuleId]);

  // Cleanup on unmount
  useEffect(() => {
    return () => {
      client.dispose();
    };
  }, [client]);

  const dispatch = useCallback((actionId: string, params?: Record<string, unknown>) => {
    if (capsuleId) {
      client.dispatchAction(capsuleId, { action_id: actionId, params });
    }
  }, [client, capsuleId]);

  const applyPatch = useCallback((patch: JSONPatch) => {
    if (capsuleId) {
      client.applyPatch(capsuleId, patch);
    }
  }, [client, capsuleId]);

  const value: IXContextValue = {
    client,
    capsuleId,
    state,
    dispatch,
    applyPatch,
  };

  return (
    <IXContext.Provider value={value}>
      {children}
    </IXContext.Provider>
  );
}

/**
 * Props for IXRenderer
 */
interface IXRendererProps {
  /** Capsule ID (uses context if not provided) */
  capsuleId?: string;
  /** Custom component mapping */
  components?: Record<string, React.ComponentType<unknown>>;
}

/**
 * IX Renderer component
 * 
 * Renders an IX capsule UI using the React renderer.
 */
export function IXRenderer({ capsuleId: propCapsuleId, components }: IXRendererProps): React.ReactElement | null {
  const ix = useIX();
  const capsuleId = propCapsuleId || ix.capsuleId;
  const [ui, setUi] = useState<UIRoot | null>(null);
  const rendererRef = useRef<ReturnType<typeof createReactRenderer> | null>(null);

  // Create renderer
  useEffect(() => {
    rendererRef.current = createReactRenderer({
      components,
      stateStore: {
        get: <T,>(path: string) => (path ? ix.state[path] : ix.state) as T | undefined,
        set: <T,>(path: string, value: T) => {
          ix.applyPatch([{ op: 'replace', path: `/${path.replace(/\./g, '/')}`, value } as import('../state/patch').JSONPatchOperation]);
        },
        subscribe: () => () => {},
        batch: () => {},
        snapshot: () => ix.state,
        restore: () => {},
        reset: () => {},
        compute: () => {},
        bind: () => {},
        getBindings: () => [],
      },
      onEvent: (name: string, payload: unknown) => {
        ix.dispatch(name, payload as Record<string, unknown>);
      },
    });

    return () => {
      rendererRef.current = null;
    };
  }, [ix, components]);

  // Fetch UI when capsuleId changes
  useEffect(() => {
    if (capsuleId && ix.client) {
      ix.client.getCapsule(capsuleId).then((capsule) => {
        setUi(capsule.ui);
      });
    }
  }, [capsuleId, ix.client]);

  if (!ui || !rendererRef.current) {
    return null;
  }

  const { UIRoot: UIRootComponent } = rendererRef.current;
  return <UIRootComponent root={ui} />;
}

/**
 * Hook to use IX capsule state
 */
export function useIXState<T>(path: string): [T | undefined, (value: T) => void] {
  const ix = useIX();
  const value = ix.state[path] as T | undefined;

  const setValue = useCallback((newValue: T) => {
    ix.applyPatch([{ op: 'replace', path: `/${path.replace(/\./g, '/')}`, value: newValue } as import('../state/patch').JSONPatchOperation]);
  }, [ix, path]);

  return [value, setValue];
}

/**
 * Hook to dispatch IX actions
 */
export function useIXAction(actionId: string): (params?: Record<string, unknown>) => void {
  const ix = useIX();

  return useCallback((params?: Record<string, unknown>) => {
    ix.dispatch(actionId, params);
  }, [ix, actionId]);
}

// Helper function
function createIXClient(config: IXCapsuleClientConfig): IXCapsuleClient {
  return new IXCapsuleClient(config);
}

// Re-export types
export type { IXCapsuleClientConfig, IXCapsuleEvent, UIRoot, JSONPatch };
export { IXCapsuleClient };
