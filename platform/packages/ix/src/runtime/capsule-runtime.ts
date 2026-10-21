/**
 * IX Capsule Runtime
 * 
 * Integration with P3.9 MCP Apps capsule infrastructure for sandboxed
 * execution of Allternit-IX UI capsules.
 */

import type { UIRoot } from '../types';
import type { StateStore } from '../state/store';
import type { JSONPatch } from '../state/patch';
import { applyPatchToStore } from '../state/patch';
import type { PolicyGate, PolicyContext } from './policy-gates';

export interface IXCapsuleConfig {
  /** Capsule ID */
  id: string;
  /** UI definition */
  ui: UIRoot;
  /** Policy gates for security */
  policyGates?: PolicyGate[];
  /** State store (isolated per capsule) */
  stateStore: StateStore;
  /** Event handler for capsule events */
  onEvent?: (event: IXCapsuleEvent) => void;
  /** State change handler */
  onStateChange?: (path: string, value: unknown) => void;
  /** Destroy handler */
  onDestroy?: () => void;
}

export interface IXCapsuleEvent {
  type: 'action' | 'state-change' | 'error' | 'lifecycle';
  capsuleId: string;
  action?: string;
  params?: Record<string, unknown>;
  path?: string;
  value?: unknown;
  error?: Error;
  lifecycle?: 'mount' | 'unmount' | 'update';
}

export interface IXCapsule {
  /** Capsule ID */
  id: string;
  /** Get current UI definition */
  getUI: () => UIRoot;
  /** Update UI with new definition */
  updateUI: (ui: UIRoot) => void;
  /** Apply state patch */
  applyPatch: (patch: JSONPatch) => void;
  /** Get state value */
  getState: (path: string) => unknown;
  /** Set state value */
  setState: (path: string, value: unknown) => void;
  /** Dispatch action */
  dispatch: (actionId: string, params?: Record<string, unknown>) => Promise<boolean>;
  /** Subscribe to capsule events */
  subscribe: (callback: (event: IXCapsuleEvent) => void) => () => void;
  /** Destroy capsule */
  destroy: () => void;
  /** Check if capsule is active */
  isActive: () => boolean;
  /** GAP-62: Get capsule metrics */
  getMetrics: () => IXCapsuleMetrics;
}

/**
 * GAP-62: Extended capsule metrics
 */
export interface IXCapsuleMetrics {
  /** Number of renders */
  renderCount: number;
  /** Number of actions dispatched */
  actionCount: number;
  /** Number of state updates */
  stateUpdateCount: number;
  /** Number of errors */
  errorCount: number;
  /** Capsule start time (ms timestamp) */
  startTime: number;
  /** Last activity time (ms timestamp) */
  lastActivity: number;
  
  /** GAP-63: CPU usage metrics */
  cpu?: {
    /** User CPU time used (milliseconds) */
    userTime: number;
    /** System CPU time used (milliseconds) */
    systemTime: number;
    /** Total CPU time (user + system) */
    totalTime: number;
    /** CPU usage percentage (approximate) */
    usagePercent: number;
  };
  
  /** GAP-63: Memory usage metrics */
  memory?: {
    /** Heap used in bytes */
    heapUsed: number;
    /** Heap total in bytes */
    heapTotal: number;
    /** External memory in bytes */
    external: number;
    /** RSS (Resident Set Size) in bytes */
    rss: number;
    /** Array buffer memory in bytes */
    arrayBuffers: number;
  };
  
  /** GAP-63: Network metrics (for capsules making network requests) */
  network?: {
    /** Number of requests made */
    requestCount: number;
    /** Number of successful responses */
    successCount: number;
    /** Number of failed requests */
    errorCount: number;
    /** Total bytes sent */
    bytesSent: number;
    /** Total bytes received */
    bytesReceived: number;
    /** Average response time (ms) */
    avgResponseTime: number;
  };
  
  /** Uptime in milliseconds */
  uptime: number;
}

/**
 * GAP-63: System metrics collector using Node.js APIs
 */
class MetricsCollector {
  private startUsage: NodeJS.CpuUsage | null = null;
  private startTime: number;
  private networkStats = {
    requestCount: 0,
    successCount: 0,
    errorCount: 0,
    bytesSent: 0,
    bytesReceived: 0,
    responseTimes: [] as number[],
  };

  constructor() {
    this.startTime = Date.now();
    // Capture initial CPU usage if available (Node.js environment)
    if (typeof process !== 'undefined' && process.cpuUsage) {
      this.startUsage = process.cpuUsage();
    }
  }

  /**
   * Collect current CPU metrics
   */
  collectCPU(): IXCapsuleMetrics['cpu'] {
    if (typeof process === 'undefined' || !process.cpuUsage) {
      return {
        userTime: 0,
        systemTime: 0,
        totalTime: 0,
        usagePercent: 0,
      };
    }

    const currentUsage = process.cpuUsage(this.startUsage || undefined);
    const userTime = currentUsage.user / 1000; // Convert microseconds to milliseconds
    const systemTime = currentUsage.system / 1000;
    const totalTime = userTime + systemTime;
    
    // Calculate percentage based on elapsed time
    const elapsedMs = Date.now() - this.startTime;
    const usagePercent = elapsedMs > 0 ? (totalTime / elapsedMs) * 100 : 0;

    return {
      userTime,
      systemTime,
      totalTime,
      usagePercent: Math.min(usagePercent, 100), // Cap at 100%
    };
  }

  /**
   * Collect current memory metrics
   */
  collectMemory(): IXCapsuleMetrics['memory'] {
    if (typeof process === 'undefined' || !process.memoryUsage) {
      return {
        heapUsed: 0,
        heapTotal: 0,
        external: 0,
        rss: 0,
        arrayBuffers: 0,
      };
    }

    const usage = process.memoryUsage();
    return {
      heapUsed: usage.heapUsed,
      heapTotal: usage.heapTotal,
      external: usage.external || 0,
      rss: usage.rss,
      arrayBuffers: usage.arrayBuffers || 0,
    };
  }

  /**
   * Record a network request
   */
  recordRequest(bytesSent: number): void {
    this.networkStats.requestCount++;
    this.networkStats.bytesSent += bytesSent;
  }

  /**
   * Record a successful response
   */
  recordResponse(bytesReceived: number, responseTime: number): void {
    this.networkStats.successCount++;
    this.networkStats.bytesReceived += bytesReceived;
    this.networkStats.responseTimes.push(responseTime);
    
    // Keep only last 100 response times for average calculation
    if (this.networkStats.responseTimes.length > 100) {
      this.networkStats.responseTimes.shift();
    }
  }

  /**
   * Record a network error
   */
  recordNetworkError(): void {
    this.networkStats.errorCount++;
  }

  /**
   * Collect network metrics
   */
  collectNetwork(): IXCapsuleMetrics['network'] {
    const avgResponseTime = this.networkStats.responseTimes.length > 0
      ? this.networkStats.responseTimes.reduce((a, b) => a + b, 0) / this.networkStats.responseTimes.length
      : 0;

    return {
      requestCount: this.networkStats.requestCount,
      successCount: this.networkStats.successCount,
      errorCount: this.networkStats.errorCount,
      bytesSent: this.networkStats.bytesSent,
      bytesReceived: this.networkStats.bytesReceived,
      avgResponseTime,
    };
  }

  /**
   * Get average response time
   */
  getAverageResponseTime(): number {
    if (this.networkStats.responseTimes.length === 0) return 0;
    return this.networkStats.responseTimes.reduce((a, b) => a + b, 0) / this.networkStats.responseTimes.length;
  }
}

/**
 * Create IX Capsule
 */
export function createIXCapsule(config: IXCapsuleConfig): IXCapsule {
  let active = true;
  let currentUI = config.ui;
  const stateStore = config.stateStore;
  const subscribers = new Set<(event: IXCapsuleEvent) => void>();
  const metricsCollector = new MetricsCollector();
  
  const metrics = {
    renderCount: 0,
    actionCount: 0,
    stateUpdateCount: 0,
    errorCount: 0,
    startTime: Date.now(),
    lastActivity: Date.now(),
  };

  /**
   * Emit event to subscribers
   */
  function emit(event: IXCapsuleEvent): void {
    if (!active) return;
    
    metrics.lastActivity = Date.now();
    subscribers.forEach((cb) => {
      try {
        cb(event);
      } catch (error) {
        console.error('IX Capsule subscriber error:', error);
      }
    });
    config.onEvent?.(event);
  }

  /**
   * Check policy gates
   */
  async function checkPolicies(
    type: 'action' | 'state-change' | 'render',
    context: Omit<PolicyContext, 'timestamp'>
  ): Promise<{ allowed: boolean; reason?: string }> {
    if (!config.policyGates || config.policyGates.length === 0) {
      return { allowed: true };
    }

    const fullContext: PolicyContext = { ...context, timestamp: Date.now() };

    for (const gate of config.policyGates) {
      const result = await gate.check(type, fullContext);
      if (!result.allowed) {
        return result;
      }
    }

    return { allowed: true };
  }

  /**
   * Get current UI
   */
  function getUI(): UIRoot {
    return currentUI;
  }

  /**
   * Update UI definition
   */
  function updateUI(ui: UIRoot): void {
    if (!active) {
      throw new Error(`Capsule ${config.id} is not active`);
    }

    currentUI = ui;
    metrics.renderCount++;
    
    emit({
      type: 'lifecycle',
      capsuleId: config.id,
      lifecycle: 'update',
    });
  }

  /**
   * Apply state patch
   */
  function applyPatch(patch: JSONPatch): void {
    if (!active) return;

    const result = applyPatchToStore(stateStore, patch);
    
    if (result.success) {
      metrics.stateUpdateCount++;
      
      // Emit state change events for affected paths
      patch.forEach((op) => {
        if (op.op !== 'test' && op.op !== 'remove') {
          const path = op.path.slice(1).replace(/\//g, '.');
          const value = stateStore.get(path);
          
          emit({
            type: 'state-change',
            capsuleId: config.id,
            path,
            value,
          });
          config.onStateChange?.(path, value);
        }
      });
    } else {
      metrics.errorCount++;
      emit({
        type: 'error',
        capsuleId: config.id,
        error: new Error(result.error || 'Patch failed'),
      });
    }
  }

  /**
   * Get state value
   */
  function getState(path: string): unknown {
    return stateStore.get(path);
  }

  /**
   * Set state value
   */
  function setState(path: string, value: unknown): void {
    if (!active) return;

    stateStore.set(path, value);
    metrics.stateUpdateCount++;
    
    emit({
      type: 'state-change',
      capsuleId: config.id,
      path,
      value,
    });
    config.onStateChange?.(path, value);
  }

  /**
   * Dispatch action with policy checking
   */
  async function dispatch(actionId: string, params?: Record<string, unknown>): Promise<boolean> {
    if (!active) {
      throw new Error(`Capsule ${config.id} is not active`);
    }

    // Find action definition
    const action = currentUI.actions.find((a) => a.id === actionId);
    if (!action) {
      const error = new Error(`Action not found: ${actionId}`);
      emit({
        type: 'error',
        capsuleId: config.id,
        error,
      });
      throw error;
    }

    // Check policies
    const policyResult = await checkPolicies('action', {
      capsuleId: config.id,
      actionId,
      params,
      state: stateStore.snapshot(),
    });

    if (!policyResult.allowed) {
      emit({
        type: 'error',
        capsuleId: config.id,
        error: new Error(`Policy denied: ${policyResult.reason}`),
      });
      return false;
    }

    metrics.actionCount++;
    
    emit({
      type: 'action',
      capsuleId: config.id,
      action: actionId,
      params,
    });

    return true;
  }

  /**
   * Subscribe to events
   */
  function subscribe(callback: (event: IXCapsuleEvent) => void): () => void {
    subscribers.add(callback);
    
    return () => {
      subscribers.delete(callback);
    };
  }

  /**
   * Destroy capsule
   */
  function destroy(): void {
    if (!active) return;

    // Emit unmount event before marking inactive
    emit({
      type: 'lifecycle',
      capsuleId: config.id,
      lifecycle: 'unmount',
    });

    active = false;
    subscribers.clear();
    config.onDestroy?.();
  }

  /**
   * Check if active
   */
  function isActive(): boolean {
    return active;
  }

  /**
   * GAP-62: Get capsule metrics
   * Returns comprehensive metrics including CPU, memory, and network stats
   */
  function getMetrics(): IXCapsuleMetrics {
    const now = Date.now();
    return {
      renderCount: metrics.renderCount,
      actionCount: metrics.actionCount,
      stateUpdateCount: metrics.stateUpdateCount,
      errorCount: metrics.errorCount,
      startTime: metrics.startTime,
      lastActivity: metrics.lastActivity,
      uptime: now - metrics.startTime,
      
      // GAP-63: Collect system metrics
      cpu: metricsCollector.collectCPU(),
      memory: metricsCollector.collectMemory(),
      network: metricsCollector.collectNetwork(),
    };
  }

  // Initialize
  emit({
    type: 'lifecycle',
    capsuleId: config.id,
    lifecycle: 'mount',
  });

  return {
    id: config.id,
    getUI,
    updateUI,
    applyPatch,
    getState,
    setState,
    dispatch,
    subscribe,
    destroy,
    isActive,
    getMetrics,
  };
}

/**
 * IX Capsule Registry
 */
export class IXCapsuleRegistry {
  private capsules = new Map<string, IXCapsule>();
  private defaultPolicyGates: PolicyGate[] = [];

  /**
   * Set default policy gates for all capsules
   */
  setDefaultPolicyGates(gates: PolicyGate[]): void {
    this.defaultPolicyGates = gates;
  }

  /**
   * Create and register capsule
   */
  create(config: Omit<IXCapsuleConfig, 'policyGates'> & { policyGates?: PolicyGate[] }): IXCapsule {
    const capsule = createIXCapsule({
      ...config,
      policyGates: config.policyGates ?? this.defaultPolicyGates,
    });

    this.capsules.set(config.id, capsule);
    return capsule;
  }

  /**
   * Get capsule by ID
   */
  get(id: string): IXCapsule | undefined {
    return this.capsules.get(id);
  }

  /**
   * Check if capsule exists
   */
  has(id: string): boolean {
    return this.capsules.has(id);
  }

  /**
   * Remove capsule
   */
  remove(id: string): boolean {
    const capsule = this.capsules.get(id);
    if (capsule) {
      capsule.destroy();
      this.capsules.delete(id);
      return true;
    }
    return false;
  }

  /**
   * List all capsules
   */
  list(): IXCapsule[] {
    return Array.from(this.capsules.values());
  }

  /**
   * Get active capsules
   */
  getActive(): IXCapsule[] {
    return this.list().filter((c) => c.isActive());
  }

  /**
   * Broadcast patch to all capsules
   */
  broadcastPatch(patch: JSONPatch): void {
    this.capsules.forEach((capsule) => {
      capsule.applyPatch(patch);
    });
  }

  /**
   * Destroy all capsules
   */
  clear(): void {
    this.capsules.forEach((capsule) => capsule.destroy());
    this.capsules.clear();
  }

  /**
   * GAP-62/64: Get metrics for all capsules
   * Returns aggregated metrics for all registered capsules
   */
  getMetrics(): Record<string, IXCapsuleMetrics> {
    const metrics: Record<string, IXCapsuleMetrics> = {};
    
    for (const [id, capsule] of this.capsules) {
      if (capsule.isActive()) {
        metrics[id] = capsule.getMetrics();
      }
    }
    
    return metrics;
  }

  /**
   * GAP-64: Get aggregated metrics across all capsules
   * Returns summary statistics for the entire registry
   */
  getAggregatedMetrics(): IXAggregatedMetrics {
    const allMetrics = Object.values(this.getMetrics());
    
    if (allMetrics.length === 0) {
      return {
        totalCapsules: 0,
        activeCapsules: 0,
        totalRenders: 0,
        totalActions: 0,
        totalStateUpdates: 0,
        totalErrors: 0,
        avgUptime: 0,
        totalMemoryUsed: 0,
        avgCpuUsage: 0,
      };
    }

    const now = Date.now();
    const activeCapsules = allMetrics.filter(m => m.uptime > 0);
    
    const totalRenders = allMetrics.reduce((sum, m) => sum + m.renderCount, 0);
    const totalActions = allMetrics.reduce((sum, m) => sum + m.actionCount, 0);
    const totalStateUpdates = allMetrics.reduce((sum, m) => sum + m.stateUpdateCount, 0);
    const totalErrors = allMetrics.reduce((sum, m) => sum + m.errorCount, 0);
    const avgUptime = activeCapsules.reduce((sum, m) => sum + m.uptime, 0) / activeCapsules.length;
    
    const totalMemoryUsed = activeCapsules.reduce(
      (sum, m) => sum + (m.memory?.heapUsed || 0), 
      0
    );
    
    const avgCpuUsage = activeCapsules.length > 0
      ? activeCapsules.reduce((sum, m) => sum + (m.cpu?.usagePercent || 0), 0) / activeCapsules.length
      : 0;

    return {
      totalCapsules: this.capsules.size,
      activeCapsules: activeCapsules.length,
      totalRenders,
      totalActions,
      totalStateUpdates,
      totalErrors,
      avgUptime,
      totalMemoryUsed,
      avgCpuUsage,
    };
  }

  /**
   * GAP-64: Export metrics for external monitoring systems
   * Returns metrics in a format suitable for Prometheus, DataDog, etc.
   */
  exportMetrics(): IXMetricsExport {
    const aggregated = this.getAggregatedMetrics();
    const individual = this.getMetrics();

    return {
      timestamp: Date.now(),
      summary: aggregated,
      capsules: individual,
      
      // Prometheus-style metrics
      prometheus: this.toPrometheusFormat(individual, aggregated),
    };
  }

  /**
   * Convert metrics to Prometheus format
   */
  private toPrometheusFormat(
    individual: Record<string, IXCapsuleMetrics>,
    aggregated: IXAggregatedMetrics
  ): string {
    const lines: string[] = [];
    
    // Help and type annotations
    lines.push('# HELP ix_capsule_renders_total Total number of renders per capsule');
    lines.push('# TYPE ix_capsule_renders_total counter');
    
    for (const [id, m] of Object.entries(individual)) {
      lines.push(`ix_capsule_renders_total{capsule_id="${id}"} ${m.renderCount}`);
    }
    
    lines.push('');
    lines.push('# HELP ix_capsule_actions_total Total number of actions per capsule');
    lines.push('# TYPE ix_capsule_actions_total counter');
    
    for (const [id, m] of Object.entries(individual)) {
      lines.push(`ix_capsule_actions_total{capsule_id="${id}"} ${m.actionCount}`);
    }
    
    lines.push('');
    lines.push('# HELP ix_capsule_errors_total Total number of errors per capsule');
    lines.push('# TYPE ix_capsule_errors_total counter');
    
    for (const [id, m] of Object.entries(individual)) {
      lines.push(`ix_capsule_errors_total{capsule_id="${id}"} ${m.errorCount}`);
    }
    
    lines.push('');
    lines.push('# HELP ix_capsule_memory_bytes Memory usage per capsule');
    lines.push('# TYPE ix_capsule_memory_bytes gauge');
    
    for (const [id, m] of Object.entries(individual)) {
      if (m.memory) {
        lines.push(`ix_capsule_memory_bytes{capsule_id="${id}",type="heap_used"} ${m.memory.heapUsed}`);
        lines.push(`ix_capsule_memory_bytes{capsule_id="${id}",type="heap_total"} ${m.memory.heapTotal}`);
        lines.push(`ix_capsule_memory_bytes{capsule_id="${id}",type="rss"} ${m.memory.rss}`);
      }
    }
    
    lines.push('');
    lines.push('# HELP ix_capsule_cpu_usage_percent CPU usage percentage per capsule');
    lines.push('# TYPE ix_capsule_cpu_usage_percent gauge');
    
    for (const [id, m] of Object.entries(individual)) {
      if (m.cpu) {
        lines.push(`ix_capsule_cpu_usage_percent{capsule_id="${id}"} ${m.cpu.usagePercent.toFixed(2)}`);
      }
    }
    
    lines.push('');
    lines.push('# HELP ix_registry_capsules_total Total number of capsules in registry');
    lines.push('# TYPE ix_registry_capsules_total gauge');
    lines.push(`ix_registry_capsules_total{state="active"} ${aggregated.activeCapsules}`);
    lines.push(`ix_registry_capsules_total{state="total"} ${aggregated.totalCapsules}`);
    
    return lines.join('\n');
  }
}

/**
 * GAP-64: Aggregated metrics interface
 */
export interface IXAggregatedMetrics {
  /** Total number of capsules in registry */
  totalCapsules: number;
  /** Number of currently active capsules */
  activeCapsules: number;
  /** Total renders across all capsules */
  totalRenders: number;
  /** Total actions across all capsules */
  totalActions: number;
  /** Total state updates across all capsules */
  totalStateUpdates: number;
  /** Total errors across all capsules */
  totalErrors: number;
  /** Average uptime in milliseconds */
  avgUptime: number;
  /** Total memory used in bytes */
  totalMemoryUsed: number;
  /** Average CPU usage percentage */
  avgCpuUsage: number;
}

/**
 * GAP-64: Metrics export format
 */
export interface IXMetricsExport {
  /** Export timestamp */
  timestamp: number;
  /** Summary statistics */
  summary: IXAggregatedMetrics;
  /** Individual capsule metrics */
  capsules: Record<string, IXCapsuleMetrics>;
  /** Prometheus-formatted metrics */
  prometheus: string;
}

/**
 * Global capsule registry
 */
export const globalCapsuleRegistry = new IXCapsuleRegistry();
