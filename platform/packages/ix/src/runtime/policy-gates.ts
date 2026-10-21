/**
 * Policy Gates
 * 
 * Security controls for IX capsule execution.
 * 
 * Policy gates enforce security boundaries for:
 * - Action execution (rate limiting, allowlists)
 * - State changes (validation, sanitization)
 * - Rendering (component restrictions, prop filtering)
 */

export interface PolicyContext {
  /** Capsule ID */
  capsuleId: string;
  /** Action being performed */
  actionId?: string;
  /** Action parameters */
  params?: Record<string, unknown>;
  /** State snapshot */
  state?: Record<string, unknown>;
  /** Component being rendered */
  component?: string;
  /** Props being passed */
  props?: Record<string, unknown>;
  /** Timestamp */
  timestamp: number;
  /** User/agent identifier */
  agentId?: string;
}

export interface PolicyResult {
  allowed: boolean;
  reason?: string;
  /** Modified context (for sanitization) */
  context?: PolicyContext;
  /** Audit log entry */
  audit?: Record<string, unknown>;
}

export interface PolicyGate {
  name: string;
  check: (type: 'action' | 'state-change' | 'render', context: PolicyContext) => Promise<PolicyResult> | PolicyResult;
}

/**
 * Rate Limiting Policy Gate
 * 
 * Limits action execution rate per capsule.
 */
export function createRateLimitGate(options: {
  maxActionsPerMinute: number;
  burstSize?: number;
  windowMs?: number;
}): PolicyGate {
  const { maxActionsPerMinute, burstSize = maxActionsPerMinute, windowMs = 60000 } = options;
  const actionLog = new Map<string, number[]>();

  return {
    name: 'rate-limit',
    check: (_type, context) => {
      if (_type !== 'action') return { allowed: true };

      const now = Date.now();
      const log = actionLog.get(context.capsuleId) || [];
      
      // Remove old entries outside window
      const validLog = log.filter((ts) => now - ts < windowMs);
      
      // Check burst limit
      if (validLog.length >= burstSize) {
        return {
          allowed: false,
          reason: `Rate limit exceeded: ${maxActionsPerMinute} actions per minute`,
        };
      }

      // Record action
      validLog.push(now);
      actionLog.set(context.capsuleId, validLog);

      return { allowed: true };
    },
  };
}

/**
 * Action Allowlist Policy Gate
 * 
 * Only allows specific actions to execute.
 */
export function createActionAllowlistGate(allowedActions: string[]): PolicyGate {
  const allowedSet = new Set(allowedActions);

  return {
    name: 'action-allowlist',
    check: (type, context) => {
      if (type !== 'action') return { allowed: true };

      if (!context.actionId || !allowedSet.has(context.actionId)) {
        return {
          allowed: false,
          reason: `Action '${context.actionId}' is not in allowlist`,
        };
      }

      return { allowed: true };
    },
  };
}

/**
 * Component Restrictions Policy Gate
 * 
 * Restricts which components can be rendered.
 */
export function createComponentRestrictionsGate(options: {
  allowedComponents?: string[];
  blockedComponents?: string[];
  maxDepth?: number;
}): PolicyGate {
  const { allowedComponents, blockedComponents = [], maxDepth = 10 } = options;
  const allowedSet = allowedComponents ? new Set(allowedComponents) : null;
  const blockedSet = new Set(blockedComponents);

  return {
    name: 'component-restrictions',
    check: (type, context) => {
      if (type !== 'render') return { allowed: true };

      if (!context.component) return { allowed: true };

      // Check blocked
      if (blockedSet.has(context.component)) {
        return {
          allowed: false,
          reason: `Component '${context.component}' is blocked`,
        };
      }

      // Check allowed
      if (allowedSet && !allowedSet.has(context.component)) {
        return {
          allowed: false,
          reason: `Component '${context.component}' is not in allowlist`,
        };
      }

      return { allowed: true };
    },
  };
}

/**
 * State Validation Policy Gate
 * 
 * Validates state changes against schema.
 */
export function createStateValidationGate(schema: {
  [path: string]: {
    type: 'string' | 'number' | 'boolean' | 'array' | 'object';
    required?: boolean;
    validate?: (value: unknown) => boolean | string;
  };
}): PolicyGate {
  return {
    name: 'state-validation',
    check: (type, context) => {
      if (type !== 'state-change') return { allowed: true };

      // This would need the actual path and value being changed
      // For now, validate entire state snapshot
      if (!context.state) return { allowed: true };

      for (const [path, rules] of Object.entries(schema)) {
        const value = getPathValue(context.state, path);

        // Check required
        if (rules.required && (value === undefined || value === null)) {
          return {
            allowed: false,
            reason: `State path '${path}' is required`,
          };
        }

        // Skip if not required and undefined
        if (!rules.required && (value === undefined || value === null)) {
          continue;
        }

        // Check type
        if (rules.type) {
          const actualType = getValueType(value);
          if (actualType !== rules.type) {
            return {
              allowed: false,
              reason: `State path '${path}' must be ${rules.type}, got ${actualType}`,
            };
          }
        }

        // Run custom validation
        if (rules.validate) {
          const result = rules.validate(value);
          if (result !== true) {
            return {
              allowed: false,
              reason: typeof result === 'string' ? result : `Validation failed for '${path}'`,
            };
          }
        }
      }

      return { allowed: true };
    },
  };
}

/**
 * Prop Sanitization Policy Gate
 * 
 * Sanitizes component props to prevent XSS and injection attacks.
 */
export function createPropSanitizationGate(options: {
  allowHTML?: boolean;
  allowedTags?: string[];
  allowedAttributes?: string[];
}): PolicyGate {
  const { allowHTML = false, allowedTags = [], allowedAttributes = [] } = options;

  return {
    name: 'prop-sanitization',
    check: (type, context) => {
      if (type !== 'render' || !context.props) return { allowed: true };

      const sanitizedProps = { ...context.props };
      let modified = false;

      for (const [key, value] of Object.entries(context.props)) {
        // Sanitize string values
        if (typeof value === 'string') {
          // Remove script tags and event handlers
          let sanitized = value;
          
          if (!allowHTML) {
            // Strip all HTML
            sanitized = sanitized.replace(/<[^>]*>/g, '');
          } else {
            // Remove script tags and their content completely
            sanitized = sanitized.replace(/<script[^>]*>[\s\S]*?<\/script>/gi, '');
            
            // Allow only specific tags
            const tagRegex = /<\/?([a-z][a-z0-9]*)[^>]*>/gi;
            sanitized = sanitized.replace(tagRegex, (match, tag) => {
              return allowedTags.includes(tag.toLowerCase()) ? match : '';
            });

            // Remove event handlers
            sanitized = sanitized.replace(/on\w+\s*=/gi, '');

            // Remove javascript: URLs
            sanitized = sanitized.replace(/javascript:/gi, '');
          }

          if (sanitized !== value) {
            sanitizedProps[key] = sanitized;
            modified = true;
          }
        }

        // Block dangerous props
        if (key.startsWith('on') && typeof value === 'string') {
          // Block string-based event handlers
          delete sanitizedProps[key];
          modified = true;
        }
      }

      return {
        allowed: true,
        context: modified ? { ...context, props: sanitizedProps } : context,
      };
    },
  };
}

/**
 * Resource Limiting Policy Gate
 * 
 * Enforces resource limits on capsules.
 */
export function createResourceLimitGate(options: {
  maxStateSize?: number; // bytes
  maxComponents?: number;
  maxActions?: number;
}): PolicyGate {
  const { maxStateSize = 1024 * 1024, maxComponents = 100, maxActions = 1000 } = options;
  const actionCounts = new Map<string, number>();

  return {
    name: 'resource-limit',
    check: (type, context) => {
      // Check state size
      if (type === 'state-change' && context.state) {
        const stateSize = JSON.stringify(context.state).length;
        if (stateSize > maxStateSize) {
          return {
            allowed: false,
            reason: `State size (${stateSize} bytes) exceeds limit (${maxStateSize} bytes)`,
          };
        }
      }

      // Check action count
      if (type === 'action') {
        const count = (actionCounts.get(context.capsuleId) || 0) + 1;
        if (count > maxActions) {
          return {
            allowed: false,
            reason: `Maximum action count (${maxActions}) exceeded`,
          };
        }
        actionCounts.set(context.capsuleId, count);
      }

      return { allowed: true };
    },
  };
}

/**
 * Audit Logging Policy Gate
 * 
 * Logs all policy decisions for audit purposes.
 */
export function createAuditLogGate(options: {
  logFn: (entry: Record<string, unknown>) => void;
  logAllowed?: boolean;
}): PolicyGate {
  const { logFn, logAllowed = false } = options;

  return {
    name: 'audit-log',
    check: (type, context) => {
      const shouldLog = logAllowed || type === 'action';
      
      if (shouldLog) {
        logFn({
          timestamp: Date.now(),
          capsuleId: context.capsuleId,
          actionId: context.actionId,
          type,
          agentId: context.agentId,
        });
      }

      return { allowed: true };
    },
  };
}

/**
 * Compose multiple policy gates
 */
export function composePolicyGates(...gates: PolicyGate[]): PolicyGate {
  return {
    name: `composed(${gates.map((g) => g.name).join(', ')})`,
    check: async (type, context) => {
      let currentContext = context;

      for (const gate of gates) {
        const result = await gate.check(type, currentContext);
        
        if (!result.allowed) {
          return result;
        }

        if (result.context) {
          currentContext = result.context;
        }
      }

      return { allowed: true, context: currentContext };
    },
  };
}

/**
 * Create default security policy gates
 */
export function createDefaultPolicyGates(): PolicyGate[] {
  return [
    createRateLimitGate({ maxActionsPerMinute: 60 }),
    createPropSanitizationGate({ allowHTML: false }),
    createResourceLimitGate({
      maxStateSize: 1024 * 1024, // 1MB
      maxComponents: 100,
      maxActions: 1000,
    }),
  ];
}

// Helper functions
function getPathValue(obj: Record<string, unknown>, path: string): unknown {
  return path.split('.').reduce((acc: unknown, key) => {
    if (acc && typeof acc === 'object') {
      return (acc as Record<string, unknown>)[key];
    }
    return undefined;
  }, obj);
}

function getValueType(value: unknown): string {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  return typeof value;
}
