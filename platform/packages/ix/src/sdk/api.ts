/**
 * Allternit-IX API SDK
 * 
 * Client for interacting with the Allternit-IX API.
 */

import type { UIRoot } from '../types';
import type { JSONPatch } from '../state/patch';

export interface IXCapsuleClientConfig {
  /** API base URL */
  baseUrl: string;
  /** Optional auth token */
  token?: string;
  /** Event callbacks */
  onEvent?: (event: IXCapsuleEvent) => void;
  /** Error handler */
  onError?: (error: Error) => void;
}

export interface IXCapsuleEvent {
  event_type: 'lifecycle' | 'action' | 'state-change' | 'error';
  capsule_id: string;
  lifecycle?: 'mount' | 'unmount' | 'update';
  action?: string;
  action_params?: Record<string, unknown>;
  state_update?: Record<string, unknown>;
  error?: string;
}

export interface CreateCapsuleRequest {
  ui: UIRoot;
  initial_state?: Record<string, unknown>;
  ttl_seconds?: number;
}

export interface CreateCapsuleResponse {
  capsule_id: string;
  status: string;
}

export interface CapsuleState {
  capsule_id: string;
  ui: UIRoot;
  state: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface JsonRenderRequest {
  schema: Record<string, unknown>;
  initial_state?: Record<string, unknown>;
  ttl_seconds?: number;
}

export interface ActionRequest {
  action_id: string;
  params?: Record<string, unknown>;
}

/**
 * Allternit-IX Capsule Client
 */
export class IXCapsuleClient {
  private config: IXCapsuleClientConfig;
  private eventSource: EventSource | null = null;
  private activeCapsules = new Set<string>();

  constructor(config: IXCapsuleClientConfig) {
    this.config = config;
  }

  /**
   * Create a new IX capsule
   */
  async createCapsule(request: CreateCapsuleRequest): Promise<CreateCapsuleResponse> {
    const response = await fetch(`${this.config.baseUrl}/ix/capsules`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(this.config.token && { 'Authorization': `Bearer ${this.config.token}` }),
      },
      body: JSON.stringify(request),
    });

    if (!response.ok) {
      throw new Error(`Failed to create capsule: ${response.statusText}`);
    }

    const result = await response.json();
    this.activeCapsules.add(result.capsule_id);
    
    // Start event stream
    this.connectEventStream(result.capsule_id);
    
    return result;
  }

  /**
   * Create capsule from json-render format
   */
  async createFromJsonRender(request: JsonRenderRequest): Promise<CreateCapsuleResponse> {
    const response = await fetch(`${this.config.baseUrl}/ix/capsules/json-render`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(this.config.token && { 'Authorization': `Bearer ${this.config.token}` }),
      },
      body: JSON.stringify(request),
    });

    if (!response.ok) {
      throw new Error(`Failed to create capsule: ${response.statusText}`);
    }

    const result = await response.json();
    this.activeCapsules.add(result.capsule_id);
    
    // Start event stream
    this.connectEventStream(result.capsule_id);
    
    return result;
  }

  /**
   * Get capsule state
   */
  async getCapsule(capsuleId: string): Promise<CapsuleState> {
    const response = await fetch(`${this.config.baseUrl}/ix/capsules/${capsuleId}`, {
      headers: {
        ...(this.config.token && { 'Authorization': `Bearer ${this.config.token}` }),
      },
    });

    if (!response.ok) {
      throw new Error(`Failed to get capsule: ${response.statusText}`);
    }

    return response.json();
  }

  /**
   * Apply JSON Patch to capsule state
   */
  async applyPatch(capsuleId: string, patch: JSONPatch): Promise<void> {
    const response = await fetch(`${this.config.baseUrl}/ix/capsules/${capsuleId}/patch`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(this.config.token && { 'Authorization': `Bearer ${this.config.token}` }),
      },
      body: JSON.stringify({ operations: patch }),
    });

    if (!response.ok) {
      throw new Error(`Failed to apply patch: ${response.statusText}`);
    }
  }

  /**
   * Dispatch action to capsule
   */
  async dispatchAction(capsuleId: string, action: ActionRequest): Promise<void> {
    const response = await fetch(`${this.config.baseUrl}/ix/capsules/${capsuleId}/action`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(this.config.token && { 'Authorization': `Bearer ${this.config.token}` }),
      },
      body: JSON.stringify(action),
    });

    if (!response.ok) {
      throw new Error(`Failed to dispatch action: ${response.statusText}`);
    }
  }

  /**
   * Delete capsule
   */
  async deleteCapsule(capsuleId: string): Promise<void> {
    const response = await fetch(`${this.config.baseUrl}/ix/capsules/${capsuleId}`, {
      method: 'DELETE',
      headers: {
        ...(this.config.token && { 'Authorization': `Bearer ${this.config.token}` }),
      },
    });

    if (!response.ok) {
      throw new Error(`Failed to delete capsule: ${response.statusText}`);
    }

    this.activeCapsules.delete(capsuleId);
    this.disconnectEventStream(capsuleId);
  }

  /**
   * Connect to event stream
   */
  private connectEventStream(capsuleId: string): void {
    if (typeof EventSource === 'undefined') {
      console.warn('EventSource not available');
      return;
    }

    const url = new URL(`${this.config.baseUrl}/ix/capsules/${capsuleId}/stream`);
    if (this.config.token) {
      url.searchParams.set('token', this.config.token);
    }

    const es = new EventSource(url.toString());
    this.eventSource = es;

    es.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data) as IXCapsuleEvent;
        this.config.onEvent?.(data);

        // Handle unmount - cleanup
        if (data.event_type === 'lifecycle' && data.lifecycle === 'unmount') {
          this.activeCapsules.delete(capsuleId);
          this.disconnectEventStream(capsuleId);
        }
      } catch (error) {
        this.config.onError?.(error as Error);
      }
    };

    es.onerror = (error) => {
      this.config.onError?.(new Error('EventSource error'));
    };
  }

  /**
   * Disconnect event stream
   */
  private disconnectEventStream(capsuleId: string): void {
    if (this.eventSource) {
      this.eventSource.close();
      this.eventSource = null;
    }
  }

  /**
   * Cleanup all connections
   */
  dispose(): void {
    this.activeCapsules.forEach((id) => {
      this.disconnectEventStream(id);
    });
    this.activeCapsules.clear();
  }
}

/**
 * Create IX capsule client
 */
export function createIXClient(config: IXCapsuleClientConfig): IXCapsuleClient {
  return new IXCapsuleClient(config);
}
