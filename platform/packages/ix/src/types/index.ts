/**
 * Allternit-IX (Allternit Interface) Types
 * 
 * Declarative UI Intermediate Representation
 */

/**
 * UI Root - Top-level document
 */
export interface UIRoot {
  /** Schema version */
  version: string;
  /** Component tree */
  components: UIComponent[];
  /** State definitions */
  state: UIState;
  /** Action handlers */
  actions: UIAction[];
  /** Custom CSS */
  css?: string;
  /** Metadata */
  metadata?: UIMetadata;
}

/**
 * UI Component
 */
export interface UIComponent {
  /** Unique component ID */
  id: string;
  /** Component type (from catalog) */
  type: string;
  /** Component properties */
  props: Record<string, unknown>;
  /** Child components */
  children?: UIComponent[];
  /** State bindings */
  bindings?: StateBinding[];
  /** Event handlers */
  events?: EventHandler[];
  /** Conditional render */
  condition?: Expression;
  /** Repeat/for loop */
  repeat?: RepeatBinding;
  /** Style overrides */
  style?: Record<string, string | number>;
  /** CSS class */
  className?: string;
}

/**
 * UI State definition
 */
export interface UIState {
  /** State variables */
  variables: StateVariable[];
  /** Computed values */
  computed?: ComputedValue[];
  /** Initial values */
  initial?: Record<string, unknown>;
}

/**
 * State variable
 */
export interface StateVariable {
  /** Variable name/path */
  path: string;
  /** Variable type */
  type: 'string' | 'number' | 'boolean' | 'array' | 'object' | 'any';
  /** Default value */
  default?: unknown;
  /** Validation rules */
  validation?: ValidationRule[];
  /** Persistence config */
  persist?: boolean | PersistenceConfig;
}

/**
 * Persistence configuration
 */
export interface PersistenceConfig {
  /** Storage key */
  key: string;
  /** Storage type */
  storage: 'localStorage' | 'sessionStorage' | 'indexedDB';
  /** Encrypt data */
  encrypt?: boolean;
  /** TTL in ms */
  ttl?: number;
}

/**
 * Computed value
 */
export interface ComputedValue {
  /** Computed value name */
  name: string;
  /** Dependencies */
  deps: string[];
  /** Compute expression */
  compute: Expression;
  /** Cache result */
  cache?: boolean;
}

/**
 * State binding
 */
export interface StateBinding {
  /** Property to bind */
  prop: string;
  /** State path */
  statePath: string;
  /** Binding direction */
  direction: 'one-way' | 'two-way';
  /** Transform function */
  transform?: Expression;
}

/**
 * Repeat/for binding
 */
export interface RepeatBinding {
  /** Array state path */
  items: string;
  /** Item variable name */
  as: string;
  /** Index variable name */
  indexAs?: string;
  /** Filter expression */
  filter?: Expression;
  /** Sort expression */
  sort?: Expression;
}

/**
 * Event handler
 */
export interface EventHandler {
  /** Event name (e.g., 'onClick', 'onChange') */
  event: string;
  /** Action to execute */
  action: string;
  /** Action parameters */
  params?: Record<string, unknown>;
  /** Condition to execute */
  condition?: Expression;
  /** Prevent default */
  preventDefault?: boolean;
  /** Stop propagation */
  stopPropagation?: boolean;
}

/**
 * UI Action
 */
export interface UIAction {
  /** Action ID */
  id: string;
  /** Action type */
  type: 'setState' | 'callFunction' | 'emitEvent' | 'navigate' | 'validate' | 'custom';
  /** Handler implementation */
  handler: ActionHandler;
  /** Async action */
  async?: boolean;
  /** Debounce ms */
  debounce?: number;
  /** Throttle ms */
  throttle?: number;
}

/**
 * Action handler
 */
export type ActionHandler = 
  | { type: 'setState'; path: string; value: Expression }
  | { type: 'callFunction'; name: string; args: Expression[] }
  | { type: 'emitEvent'; name: string; payload: Expression }
  | { type: 'navigate'; to: string; params?: Record<string, Expression> }
  | { type: 'validate'; paths: string[] }
  | { type: 'custom'; fn: string };  // fn is the handler name in the registry

/**
 * Custom handler execution context
 */
export interface CustomHandlerContext {
  /** Current state */
  state: Record<string, unknown>;
  /** Action parameters */
  params?: Record<string, unknown>;
  /** Original event */
  event?: unknown;
  /** Set state helper */
  setState: (path: string, value: unknown) => void;
  /** Get state helper */
  getState: (path: string) => unknown;
}

/**
 * Custom handler function type
 */
export type CustomHandlerFn = (context: CustomHandlerContext) => void | Promise<void>;

/**
 * Expression - can be evaluated
 */
export type Expression = 
  | string      // Path or literal
  | number      // Literal
  | boolean     // Literal
  | null        // Literal
  | { $expr: string; $deps?: string[] }  // Computed expression
  | { $path: string }  // State path reference
  | { $fn: string; $args: Expression[] }  // Function call
  | { $cond: [Expression, Expression, Expression] }  // Ternary
  | { $and: Expression[] }  // Logical AND
  | { $or: Expression[] }  // Logical OR
  | { $not: Expression }  // Logical NOT
  | { $eq: [Expression, Expression] }  // Equality
  | { $gt: [Expression, Expression] }  // Greater than
  | { $lt: [Expression, Expression] }  // Less than
  | { $add: [Expression, Expression] }  // Addition
  | { $concat: Expression[] };  // String concatenation

/**
 * Validation rule
 */
export interface ValidationRule {
  /** Rule type */
  type: 'required' | 'min' | 'max' | 'minLength' | 'maxLength' | 'pattern' | 'custom';
  /** Rule value */
  value?: unknown;
  /** Error message */
  message: string;
}

/**
 * UI Metadata
 */
export interface UIMetadata {
  /** Document title */
  title?: string;
  /** Document description */
  description?: string;
  /** Author */
  author?: string;
  /** Created timestamp */
  createdAt?: string;
  /** Last updated */
  updatedAt?: string;
  /** Tags */
  tags?: string[];
}

/**
 * Component catalog entry
 */
export interface CatalogEntry {
  /** Component name */
  name: string;
  /** Component version */
  version: string;
  /** Component category */
  category: string;
  /** Prop schema */
  props: PropSchema[];
  /** Description */
  description?: string;
  /** Example usage */
  examples?: UIComponent[];
  /** Icon */
  icon?: string;
  /** Whether component accepts children */
  acceptChildren?: boolean;
  /** Required parent types */
  requiredParent?: string[];
}

/**
 * Property schema
 */
export interface PropSchema {
  /** Property name */
  name: string;
  /** Property type */
  type: 'string' | 'number' | 'boolean' | 'array' | 'object' | 'function' | 'any';
  /** Required */
  required?: boolean;
  /** Default value */
  default?: unknown;
  /** Allowed values (for enums) */
  enum?: unknown[];
  /** Property description */
  description?: string;
  /** Validation */
  validation?: ValidationRule[];
}

/**
 * JSON Patch operation (RFC 6902)
 */
export interface JSONPatch {
  /** Operation type */
  op: 'add' | 'remove' | 'replace' | 'move' | 'copy' | 'test';
  /** Target path */
  path: string;
  /** Value (for add/replace/test) */
  value?: unknown;
  /** Source path (for move/copy) */
  from?: string;
}

/**
 * UI Update patch
 */
export interface UIPatch {
  /** Patch ID */
  id: string;
  /** Timestamp */
  timestamp: number;
  /** Patches to apply */
  patches: JSONPatch[];
  /** Source (who made the change) */
  source?: string;
  /** Change description */
  description?: string;
}

/**
 * Evaluate expression
 */
export function evaluateExpression(
  expr: Expression,
  context: Record<string, unknown>
): unknown {
  // String path reference
  if (typeof expr === 'string') {
    return getValueByPath(context, expr) ?? expr;
  }

  // Number literal
  if (typeof expr === 'number') {
    return expr;
  }

  // Boolean literal
  if (typeof expr === 'boolean') {
    return expr;
  }

  // Null literal
  if (expr === null) {
    return null;
  }

  // Object expressions
  if (typeof expr === 'object') {
    // State path reference
    if ('$path' in expr) {
      return getValueByPath(context, expr.$path);
    }

    // Expression
    if ('$expr' in expr) {
      // Simple expression evaluation (would use proper parser in production)
      return evaluateSimpleExpr(expr.$expr, context);
    }

    // Function call
    if ('$fn' in expr) {
      const fn = context[expr.$fn] as (...args: unknown[]) => unknown;
      if (typeof fn === 'function') {
        const args = expr.$args.map((arg) => evaluateExpression(arg, context));
        return fn(...args);
      }
      return undefined;
    }

    // Ternary
    if ('$cond' in expr) {
      const [condition, trueValue, falseValue] = expr.$cond;
      return evaluateExpression(condition, context)
        ? evaluateExpression(trueValue, context)
        : evaluateExpression(falseValue, context);
    }

    // Logical AND
    if ('$and' in expr) {
      return expr.$and.every((e) => evaluateExpression(e, context));
    }

    // Logical OR
    if ('$or' in expr) {
      return expr.$or.some((e) => evaluateExpression(e, context));
    }

    // Logical NOT
    if ('$not' in expr) {
      return !evaluateExpression(expr.$not, context);
    }

    // Equality
    if ('$eq' in expr) {
      const [a, b] = expr.$eq;
      return evaluateExpression(a, context) === evaluateExpression(b, context);
    }

    // Greater than
    if ('$gt' in expr) {
      const [a, b] = expr.$gt;
      return (evaluateExpression(a, context) as number) > (evaluateExpression(b, context) as number);
    }

    // Less than
    if ('$lt' in expr) {
      const [a, b] = expr.$lt;
      return (evaluateExpression(a, context) as number) < (evaluateExpression(b, context) as number);
    }

    // Addition
    if ('$add' in expr) {
      const [a, b] = expr.$add;
      return (evaluateExpression(a, context) as number) + (evaluateExpression(b, context) as number);
    }

    // Concatenation
    if ('$concat' in expr) {
      return expr.$concat.map((e) => String(evaluateExpression(e, context))).join('');
    }
  }

  return undefined;
}

/**
 * Get value by path
 */
function getValueByPath(obj: Record<string, unknown>, path: string): unknown {
  return path.split('.').reduce((acc: unknown, key) => {
    if (acc && typeof acc === 'object') {
      return (acc as Record<string, unknown>)[key];
    }
    return undefined;
  }, obj);
}

/**
 * Evaluate simple expression
 */
function evaluateSimpleExpr(expr: string, context: Record<string, unknown>): unknown {
  // Replace state paths with values
  const processed = expr.replace(/\{\{([^}]+)\}\}/g, (match, path) => {
    const value = getValueByPath(context, path.trim());
    return value !== undefined ? JSON.stringify(value) : match;
  });

  try {
    // eslint-disable-next-line no-new-func
    return new Function('context', `return ${processed}`)(context);
  } catch {
    return processed;
  }
}

/** Schema version */
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
