/**
 * LLM-to-IX Pipeline
 *
 * Converts LLM-generated UI descriptions to Allternit-IX format.
 *
 * Supports multiple input formats:
 * - Natural language descriptions
 * - Vercel Labs json-render
 * - JSX-like pseudo-code
 * - Raw component trees
 * - Markdown UI descriptions
 *
 * @example
 * ```typescript
 * import { llmToIxPipeline } from '@allternit/ix/pipeline';
 *
 * const result = llmToIxPipeline({
 *   input: llmOutput,
 *   options: {
 *     applyStyling: true,
 *     theme: 'default',
 *     generateIds: true,
 *   }
 * });
 *
 * console.log(result.tree); // IxRenderTree
 * ```
 */

import type { UIRoot, UIComponent, StateVariable, UIState, UIAction, StateBinding, EventHandler, Expression, RepeatBinding } from '../types';
import { jsonRenderAdapter } from '../adapters/json-render';
import type { JsonRenderSchema } from '../adapters/json-render';
import { BUILT_INS } from '../catalog/registry';

export interface LLMToIXOptions {
  /** Schema version to use */
  version?: string;
  /** Auto-generate component IDs */
  generateIds?: boolean;
  /** Validate against catalog */
  validate?: boolean;
  /** Infer state from bindings */
  inferState?: boolean;
}

export interface LLMToIXResult {
  /** Generated UI */
  ui: UIRoot;
  /** Warnings during conversion */
  warnings: string[];
  /** Inferred state variables */
  inferredState: StateVariable[];
  /** Confidence score (0-1) */
  confidence: number;
}

/**
 * Allternit-IX Theme Configuration
 */
export interface IxTheme {
  /** Theme name */
  name: string;
  /** Color palette */
  colors: {
    primary: string;
    secondary: string;
    success: string;
    warning: string;
    error: string;
    background: string;
    surface: string;
    text: string;
    textMuted: string;
    border: string;
  };
  /** Spacing scale */
  spacing: {
    xs: number;
    sm: number;
    md: number;
    lg: number;
    xl: number;
  };
  /** Typography */
  typography: {
    fontFamily: string;
    fontSize: {
      xs: string;
      sm: string;
      md: string;
      lg: string;
      xl: string;
    };
    fontWeight: {
      normal: number;
      medium: number;
      semibold: number;
      bold: number;
    };
  };
  /** Border radius */
  borderRadius: {
    sm: number;
    md: number;
    lg: number;
    full: number;
  };
  /** Shadows */
  shadows: {
    sm: string;
    md: string;
    lg: string;
  };
}

/**
 * IxRenderTree - Render-ready component tree with styling applied
 */
export interface IxRenderTree {
  /** Schema version */
  version: string;
  /** Root components */
  components: IxRenderNode[];
  /** State definitions */
  state: UIState;
  /** Action handlers */
  actions: UIAction[];
  /** Applied theme */
  theme?: IxTheme;
  /** CSS styles */
  styles?: Record<string, Record<string, string | number>>;
  /** Metadata */
  metadata?: {
    title?: string;
    description?: string;
    generatedAt?: string;
  };
}

/**
 * IxRenderNode - Styled component node
 */
export interface IxRenderNode {
  /** Unique node ID */
  id: string;
  /** Component type */
  type: string;
  /** Component props */
  props: Record<string, unknown>;
  /** Applied styles */
  style: Record<string, string | number>;
  /** CSS classes */
  className?: string;
  /** Child nodes */
  children?: IxRenderNode[];
  /** State bindings */
  bindings?: StateBinding[];
  /** Event handlers */
  events?: EventHandler[];
  /** Conditional render */
  condition?: Expression;
  /** Repeat/for loop */
  repeat?: RepeatBinding;
}

/**
 * Pipeline Options for llmToIxPipeline
 */
export interface LlmToIxPipelineOptions {
  /** LLM output to parse */
  input: unknown;
  /** Pipeline options */
  options?: {
    /** Schema version */
    version?: string;
    /** Auto-generate component IDs */
    generateIds?: boolean;
    /** Validate against catalog */
    validate?: boolean;
    /** Infer state from bindings */
    inferState?: boolean;
    /** Apply Allternit-IX styling */
    applyStyling?: boolean;
    /** Theme to use */
    theme?: 'default' | 'minimal' | 'compact' | IxTheme;
    /** Output format */
    outputFormat?: 'tree' | 'ui-root' | 'json-render';
    /** Debug mode */
    debug?: boolean;
  };
}

/**
 * Pipeline Result
 */
export interface LlmToIxPipelineResult {
  /** Render tree */
  tree: IxRenderTree;
  /** Original UI root */
  uiRoot: UIRoot;
  /** Warnings */
  warnings: string[];
  /** Inferred state */
  inferredState: StateVariable[];
  /** Confidence score */
  confidence: number;
  /** Processing metrics */
  metrics: {
    parseTime: number;
    transformTime: number;
    styleTime: number;
    totalTime: number;
  };
}

/**
 * Convert LLM output to Allternit-IX
 */
export function convertLLMToIX(
  input: unknown,
  options: LLMToIXOptions = {}
): LLMToIXResult {
  const warnings: string[] = [];
  const inferredState: StateVariable[] = [];

  // Detect input format
  const format = detectInputFormat(input);
  
  switch (format) {
    case 'json-render':
      return convertFromJsonRender(input as JsonRenderSchema, options);
    
    case 'jsx-like':
      return convertFromJSXLike(input as string, options);
    
    case 'component-tree':
      return convertFromComponentTree(input as Record<string, unknown>, options);
    
    case 'natural-language':
      return convertFromNaturalLanguage(input as string, options);
    
    default:
      warnings.push('Unknown input format, attempting generic conversion');
      return convertGeneric(input, options);
  }
}

/**
 * Detect input format
 */
function detectInputFormat(input: unknown): 'json-render' | 'jsx-like' | 'component-tree' | 'natural-language' | 'unknown' {
  if (!input) return 'unknown';

  // Check for json-render
  if (jsonRenderAdapter.isValidSchema(input)) {
    return 'json-render';
  }

  // Check for JSX-like (string with JSX syntax)
  if (typeof input === 'string') {
    const trimmed = input.trim();
    if (trimmed.startsWith('<') && trimmed.endsWith('>')) {
      return 'jsx-like';
    }
    if (trimmed.length >= 40 && !trimmed.startsWith('{')) {
      return 'natural-language';
    }
  }

  // Check for component tree object (including empty object)
  if (typeof input === 'object' && input !== null && !Array.isArray(input)) {
    const obj = input as Record<string, unknown>;
    if (obj.components || obj.type || obj.root || Object.keys(obj).length === 0) {
      return 'component-tree';
    }
  }

  return 'unknown';
}

/**
 * Convert from json-render format
 */
function convertFromJsonRender(
  schema: JsonRenderSchema,
  options: LLMToIXOptions
): LLMToIXResult {
  const warnings: string[] = [];
  const inferredState: StateVariable[] = [];

  const ui = jsonRenderAdapter.fromJsonRender(schema);

  // Add IDs if needed
  if (options.generateIds) {
    addComponentIds(ui.components);
  }

  // Infer state from bindings
  if (options.inferState) {
    inferStateFromBindings(ui, inferredState);
  }

  return {
    ui,
    warnings,
    inferredState,
    confidence: 0.95,
  };
}

/**
 * Convert from JSX-like string
 */
function convertFromJSXLike(
  jsxString: string,
  options: LLMToIXOptions
): LLMToIXResult {
  const warnings: string[] = [];
  const inferredState: StateVariable[] = [];

  try {
    const parsed = parseJSXLike(jsxString);
    
    // Check if parsing produced any components
    if (parsed.length === 0) {
      warnings.push('No valid JSX components found');
      return {
        ui: createEmptyUI(options.version),
        warnings,
        inferredState,
        confidence: 0.3,
      };
    }
    
    const components = parsed.map((node) => jsxNodeToComponent(node));

    const ui: UIRoot = {
      version: options.version || '1.0.0',
      components,
      state: { variables: [] },
      actions: [],
    };

    // Infer state
    if (options.inferState) {
      inferStateFromBindings(ui, inferredState);
    }

    return {
      ui,
      warnings,
      inferredState,
      confidence: 0.85,
    };
  } catch (error) {
    warnings.push(`Failed to parse JSX: ${error}`);
    return {
      ui: createEmptyUI(options.version),
      warnings,
      inferredState,
      confidence: 0.3,
    };
  }
}

/**
 * Parse JSX-like string to AST
 */
function parseJSXLike(jsx: string): JSXNode[] {
  // Simple JSX parser - in production, use a proper parser
  const nodes: JSXNode[] = [];
  const tagRegex = /<(\w+)([^>]*)>([\s\S]*?)<\/\1>|<(\w+)([^>]*)\/>/g;
  
  let match;
  while ((match = tagRegex.exec(jsx)) !== null) {
    const type = (match[1] || match[4]) ?? 'div';
    const propsString = match[2] || match[5] || '';
    const children = match[3]?.trim();

    const props = parseProps(propsString);
    const childNodes = children ? parseJSXLike(children) : [];

    nodes.push({
      type,
      props,
      children: childNodes,
      text: childNodes.length === 0 ? children : undefined,
    });
  }

  return nodes;
}

/**
 * Parse props string
 */
function parseProps(propsString: string): Record<string, unknown> {
  const props: Record<string, unknown> = {};
  // Support $ prefix for bindings: $value={path}
  const propRegex = /(\$?\w+)={([^}]*)}|(\$?\w+)="([^"]*)"|(\$?\w+)='([^']*)'|(\$?\w+)/g;
  
  let match;
  while ((match = propRegex.exec(propsString)) !== null) {
    const key = (match[1] || match[3] || match[5] || match[7]) ?? '';
    if (!key) continue;
    
    let value: unknown = true; // Boolean prop
    
    if (match[2]) {
      // Expression: prop={value}
      try {
        value = JSON.parse(match[2]);
      } catch {
        value = match[2]; // Raw string for expressions like user.name
      }
    } else if (match[4]) {
      // Double quoted: prop="value"
      value = match[4];
    } else if (match[6]) {
      // Single quoted: prop='value'
      value = match[6];
    }
    
    props[key] = value;
  }

  return props;
}

interface JSXNode {
  type: string;
  props: Record<string, unknown>;
  children: JSXNode[];
  text?: string;
}

/**
 * Convert JSX node to UIComponent
 */
function jsxNodeToComponent(node: JSXNode, parentId = 'root'): UIComponent {
  const id = generateId();
  const component: UIComponent = {
    id,
    type: node.type,
    props: {},
  };

  // Process props
  const bindings: UIComponent['bindings'] = [];
  const events: UIComponent['events'] = [];

  for (const [key, value] of Object.entries(node.props)) {
    // Check for binding (starts with $)
    if (key.startsWith('$')) {
      const propName = key.slice(1);
      // Handle both string values and object expressions
      const statePath = typeof value === 'string' 
        ? value 
        : (value as { $expr?: string })?.$expr || String(value);
      bindings.push({
        prop: propName,
        statePath,
        direction: 'one-way',
      });
    }
    // Check for event handler
    else if (key.startsWith('on')) {
      events.push({
        event: key,
        action: value as string,
      });
    }
    // Regular prop
    else {
      component.props[key] = value;
    }
  }

  if (bindings.length > 0) {
    component.bindings = bindings;
  }
  if (events.length > 0) {
    component.events = events;
  }

  // Process children
  if (node.children.length > 0) {
    component.children = node.children.map((child) => jsxNodeToComponent(child, id));
  } else if (node.text) {
    component.props.children = node.text;
  }

  return component;
}

/**
 * Convert from component tree object
 */
function convertFromComponentTree(
  tree: Record<string, unknown>,
  options: LLMToIXOptions
): LLMToIXResult {
  const warnings: string[] = [];
  const inferredState: StateVariable[] = [];

  try {
    // Handle empty object
    if (Object.keys(tree).length === 0) {
      return {
        ui: createEmptyUI(options.version),
        warnings: ['Empty input provided'],
        inferredState,
        confidence: 0.1,
      };
    }
    
    const components = tree.components
      ? (tree.components as unknown[]).map((c) => convertTreeNode(c as Record<string, unknown>))
      : tree.type 
        ? [convertTreeNode(tree)]
        : [];

    const ui: UIRoot = {
      version: options.version || '1.0.0',
      components,
      state: { variables: [] },
      actions: [],
    };

    return {
      ui,
      warnings,
      inferredState,
      confidence: 0.8,
    };
  } catch (error) {
    warnings.push(`Failed to convert tree: ${error}`);
    return {
      ui: createEmptyUI(options.version),
      warnings,
      inferredState,
      confidence: 0.4,
    };
  }
}

/**
 * Convert tree node to component
 */
function convertTreeNode(node: Record<string, unknown>): UIComponent {
  const component: UIComponent = {
    id: generateId(),
    type: (node.type as string) || 'Box',
    props: (node.props as Record<string, unknown>) || {},
  };

  if (node.children) {
    component.children = (node.children as unknown[]).map((c) =>
      convertTreeNode(c as Record<string, unknown>)
    );
  }

  if (node.bindings) {
    component.bindings = node.bindings as UIComponent['bindings'];
  }

  if (node.events) {
    component.events = node.events as UIComponent['events'];
  }

  return component;
}

/**
 * Convert from natural language
 */
function convertFromNaturalLanguage(
  description: string,
  options: LLMToIXOptions
): LLMToIXResult {
  const warnings: string[] = [];
  const inferredState: StateVariable[] = [];

  warnings.push('Natural language conversion requires LLM assistance');

  // This would typically call an LLM to generate the UI
  // For now, return a placeholder
  const ui: UIRoot = {
    version: options.version || '1.0.0',
    components: [
      {
        id: generateId(),
        type: 'Text',
        props: { children: 'Natural language conversion not yet implemented' },
      },
    ],
    state: { variables: [] },
    actions: [],
  };

  return {
    ui,
    warnings,
    inferredState,
    confidence: 0.1,
  };
}

/**
 * Generic conversion fallback
 */
function convertGeneric(
  input: unknown,
  options: LLMToIXOptions
): LLMToIXResult {
  const warnings: string[] = ['Using generic conversion'];
  const inferredState: StateVariable[] = [];

  // Try to coerce to component structure
  let components: UIComponent[] = [];

  if (Array.isArray(input)) {
    components = input.map((item, i) => ({
      id: `comp-${i}`,
      type: (item?.type as string) || 'Box',
      props: (item?.props as Record<string, unknown>) || {},
    }));
  } else if (typeof input === 'object' && input !== null) {
    components = [{
      id: generateId(),
      type: (input as Record<string, unknown>).type as string || 'Box',
      props: (input as Record<string, unknown>).props as Record<string, unknown> || {},
    }];
  }

  const ui: UIRoot = {
    version: options.version || '1.0.0',
    components,
    state: { variables: [] },
    actions: [],
  };

  return {
    ui,
    warnings,
    inferredState,
    confidence: 0.5,
  };
}

/**
 * Add component IDs
 */
function addComponentIds(components: UIComponent[], prefix = 'comp'): void {
  components.forEach((comp, i) => {
    if (!comp.id) {
      comp.id = `${prefix}-${i}`;
    }
    if (comp.children) {
      addComponentIds(comp.children, comp.id);
    }
  });
}

/**
 * Infer state from bindings
 */
function inferStateFromBindings(ui: UIRoot, inferredState: StateVariable[]): void {
  const statePaths = new Set<string>();

  function scanComponent(comp: UIComponent): void {
    comp.bindings?.forEach((binding) => {
      statePaths.add(binding.statePath);
    });
    comp.children?.forEach(scanComponent);
  }

  ui.components.forEach(scanComponent);

  statePaths.forEach((path) => {
    const existing = ui.state.variables.find((v) => v.path === path);
    if (!existing) {
      inferredState.push({
        path,
        type: 'any',
      });
    }
  });
}

/**
 * Generate unique ID
 */
function generateId(): string {
  return `comp_${Math.random().toString(36).substr(2, 9)}`;
}

/**
 * Create empty UI
 */
function createEmptyUI(version?: string): UIRoot {
  return {
    version: version || '1.0.0',
    components: [],
    state: { variables: [] },
    actions: [],
  };
}

/**
 * Allternit-IX Built-in Themes
 */
const IX_THEMES: Record<string, IxTheme> = {
  default: {
    name: 'default',
    colors: {
      primary: '#3B82F6',
      secondary: '#6B7280',
      success: '#10B981',
      warning: '#F59E0B',
      error: '#EF4444',
      background: '#FFFFFF',
      surface: '#F9FAFB',
      text: '#111827',
      textMuted: '#6B7280',
      border: '#E5E7EB',
    },
    spacing: {
      xs: 4,
      sm: 8,
      md: 16,
      lg: 24,
      xl: 32,
    },
    typography: {
      fontFamily: 'system-ui, -apple-system, sans-serif',
      fontSize: {
        xs: '12px',
        sm: '14px',
        md: '16px',
        lg: '18px',
        xl: '24px',
      },
      fontWeight: {
        normal: 400,
        medium: 500,
        semibold: 600,
        bold: 700,
      },
    },
    borderRadius: {
      sm: 4,
      md: 8,
      lg: 12,
      full: 9999,
    },
    shadows: {
      sm: '0 1px 2px rgba(0, 0, 0, 0.05)',
      md: '0 4px 6px rgba(0, 0, 0, 0.1)',
      lg: '0 10px 15px rgba(0, 0, 0, 0.1)',
    },
  },
  minimal: {
    name: 'minimal',
    colors: {
      primary: '#000000',
      secondary: '#666666',
      success: '#00AA00',
      warning: '#FFAA00',
      error: '#FF0000',
      background: '#FFFFFF',
      surface: '#FFFFFF',
      text: '#000000',
      textMuted: '#666666',
      border: '#DDDDDD',
    },
    spacing: {
      xs: 4,
      sm: 8,
      md: 12,
      lg: 16,
      xl: 24,
    },
    typography: {
      fontFamily: 'system-ui, -apple-system, sans-serif',
      fontSize: {
        xs: '11px',
        sm: '13px',
        md: '15px',
        lg: '17px',
        xl: '22px',
      },
      fontWeight: {
        normal: 400,
        medium: 500,
        semibold: 600,
        bold: 700,
      },
    },
    borderRadius: {
      sm: 0,
      md: 0,
      lg: 0,
      full: 9999,
    },
    shadows: {
      sm: 'none',
      md: 'none',
      lg: 'none',
    },
  },
  compact: {
    name: 'compact',
    colors: {
      primary: '#2563EB',
      secondary: '#475569',
      success: '#059669',
      warning: '#D97706',
      error: '#DC2626',
      background: '#FAFAFA',
      surface: '#FFFFFF',
      text: '#18181B',
      textMuted: '#71717A',
      border: '#E4E4E7',
    },
    spacing: {
      xs: 2,
      sm: 4,
      md: 8,
      lg: 12,
      xl: 16,
    },
    typography: {
      fontFamily: 'system-ui, -apple-system, sans-serif',
      fontSize: {
        xs: '10px',
        sm: '12px',
        md: '14px',
        lg: '16px',
        xl: '20px',
      },
      fontWeight: {
        normal: 400,
        medium: 500,
        semibold: 600,
        bold: 700,
      },
    },
    borderRadius: {
      sm: 2,
      md: 4,
      lg: 6,
      full: 9999,
    },
    shadows: {
      sm: '0 1px 1px rgba(0, 0, 0, 0.03)',
      md: '0 2px 4px rgba(0, 0, 0, 0.06)',
      lg: '0 4px 8px rgba(0, 0, 0, 0.08)',
    },
  },
};

/**
 * Get theme by name or return custom theme
 */
function getTheme(theme: string | IxTheme): IxTheme {
  if (typeof theme === 'string') {
    return IX_THEMES[theme] || IX_THEMES.default;
  }
  return theme;
}

/**
 * Apply Allternit-IX styling to component tree
 */
function applyStylingToTree(
  components: UIComponent[],
  theme: IxTheme
): IxRenderNode[] {
  return components.map((comp) => applyStylingToNode(comp, theme));
}

/**
 * Apply Allternit-IX styling to a single node
 */
function applyStylingToNode(comp: UIComponent, theme: IxTheme): IxRenderNode {
  const baseStyle: Record<string, string | number> = {};
  let className = '';

  // Apply component-specific default styles based on type
  const componentType = comp.type.toLowerCase();

  // Layout components
  if (componentType === 'box' || componentType === 'stack' || componentType === 'flex') {
    baseStyle.display = 'flex';
    baseStyle.flexDirection = (comp.props.flexDirection as string) || 'column';
    baseStyle.gap = theme.spacing.sm;
    baseStyle.padding = theme.spacing.md;
  }

  if (componentType === 'stack') {
    const direction = comp.props.direction as string;
    baseStyle.flexDirection = direction === 'horizontal' ? 'row' : 'column';
    baseStyle.alignItems = comp.props.align as string || 'stretch';
    baseStyle.justifyContent = comp.props.justify as string || 'flex-start';
    baseStyle.gap = (comp.props.spacing as number) || theme.spacing.sm;
  }

  // Typography components
  if (componentType === 'text') {
    baseStyle.fontSize = getFontSize(comp.props.variant as string, theme);
    baseStyle.color = theme.colors.text;
    baseStyle.fontFamily = theme.typography.fontFamily;
    baseStyle.lineHeight = 1.5;
  }

  if (componentType === 'heading') {
    const level = (comp.props.level as number) || 1;
    baseStyle.fontSize = level === 1 ? theme.typography.fontSize.xl :
                         level === 2 ? theme.typography.fontSize.lg :
                         theme.typography.fontSize.md;
    baseStyle.fontWeight = theme.typography.fontWeight.semibold;
    baseStyle.color = theme.colors.text;
    baseStyle.marginBottom = theme.spacing.md;
  }

  if (componentType === 'paragraph') {
    baseStyle.fontSize = theme.typography.fontSize.md;
    baseStyle.color = theme.colors.text;
    baseStyle.lineHeight = 1.6;
    baseStyle.marginBottom = theme.spacing.md;
  }

  // Input components
  if (componentType === 'button') {
    const variant = (comp.props.variant as string) || 'primary';
    baseStyle.padding = `${theme.spacing.sm} ${theme.spacing.md}`;
    baseStyle.borderRadius = theme.borderRadius.md;
    baseStyle.fontSize = theme.typography.fontSize.sm;
    baseStyle.fontWeight = theme.typography.fontWeight.medium;
    baseStyle.cursor = 'pointer';
    baseStyle.border = 'none';
    baseStyle.transition = 'all 0.2s ease';

    if (variant === 'primary') {
      baseStyle.backgroundColor = theme.colors.primary;
      baseStyle.color = '#FFFFFF';
    } else if (variant === 'secondary') {
      baseStyle.backgroundColor = theme.colors.surface;
      baseStyle.color = theme.colors.text;
      baseStyle.border = `1px solid ${theme.colors.border}`;
    } else if (variant === 'ghost') {
      baseStyle.backgroundColor = 'transparent';
      baseStyle.color = theme.colors.primary;
    } else if (variant === 'danger') {
      baseStyle.backgroundColor = theme.colors.error;
      baseStyle.color = '#FFFFFF';
    }
  }

  if (componentType === 'input' || componentType === 'textarea') {
    baseStyle.padding = `${theme.spacing.sm} ${theme.spacing.md}`;
    baseStyle.borderRadius = theme.borderRadius.md;
    baseStyle.border = `1px solid ${theme.colors.border}`;
    baseStyle.fontSize = theme.typography.fontSize.sm;
    baseStyle.fontFamily = theme.typography.fontFamily;
    baseStyle.outline = 'none';
    baseStyle.transition = 'border-color 0.2s ease';
    baseStyle.width = '100%';
    baseStyle.boxSizing = 'border-box';
  }

  // Display components
  if (componentType === 'card') {
    baseStyle.backgroundColor = theme.colors.surface;
    baseStyle.borderRadius = (comp.props.borderRadius as number) !== undefined
      ? `${comp.props.borderRadius}px`
      : `${theme.borderRadius.lg}px`;
    baseStyle.padding = (comp.props.padding as number) !== undefined
      ? `${comp.props.padding}px`
      : `${theme.spacing.lg}px`;
    const elevation = (comp.props.elevation as number) || 1;
    baseStyle.boxShadow = elevation === 0 ? 'none' :
                          elevation === 1 ? theme.shadows.sm :
                          elevation === 2 ? theme.shadows.md :
                          theme.shadows.lg;
  }

  if (componentType === 'badge') {
    const variant = (comp.props.variant as string) || 'default';
    baseStyle.display = 'inline-flex';
    baseStyle.alignItems = 'center';
    baseStyle.padding = `${theme.spacing.xs} ${theme.spacing.sm}`;
    baseStyle.borderRadius = theme.borderRadius.full;
    baseStyle.fontSize = theme.typography.fontSize.xs;
    baseStyle.fontWeight = theme.typography.fontWeight.medium;

    const variantColors: Record<string, { bg: string; color: string }> = {
      default: { bg: theme.colors.surface, color: theme.colors.text },
      success: { bg: '#D1FAE5', color: '#065F46' },
      warning: { bg: '#FEF3C7', color: '#92400E' },
      error: { bg: '#FEE2E2', color: '#991B1B' },
      info: { bg: '#DBEAFE', color: '#1E40AF' },
    };

    const colors = variantColors[variant] || variantColors.default;
    baseStyle.backgroundColor = colors.bg;
    baseStyle.color = colors.color;
  }

  if (componentType === 'image') {
    baseStyle.maxWidth = '100%';
    baseStyle.height = 'auto';
    baseStyle.objectFit = (comp.props.objectFit as string) || 'cover';
    baseStyle.borderRadius = theme.borderRadius.md;
  }

  // Merge with component-specific styles
  const mergedStyle = { ...baseStyle, ...(comp.style || {}) };

  // Handle style props
  if (comp.props.backgroundColor) {
    mergedStyle.backgroundColor = comp.props.backgroundColor as string;
  }
  if (comp.props.borderRadius) {
    mergedStyle.borderRadius = `${comp.props.borderRadius}px`;
  }
  if (comp.props.padding) {
    mergedStyle.padding = `${comp.props.padding}px`;
  }
  if (comp.props.margin) {
    mergedStyle.margin = `${comp.props.margin}px`;
  }
  if (comp.props.width) {
    mergedStyle.width = comp.props.width as string | number;
  }
  if (comp.props.height) {
    mergedStyle.height = comp.props.height as string | number;
  }

  // Build className from props
  const classNameParts: string[] = [];
  if (comp.props.className) {
    classNameParts.push(comp.props.className as string);
  }
  className = classNameParts.join(' ') || undefined;

  // Process children
  const children = comp.children
    ? comp.children.map((child) => applyStylingToNode(child, theme))
    : undefined;

  return {
    id: comp.id,
    type: comp.type,
    props: comp.props,
    style: mergedStyle,
    className,
    children,
    bindings: comp.bindings,
    events: comp.events,
    condition: comp.condition,
    repeat: comp.repeat,
  };
}

/**
 * Get font size based on variant
 */
function getFontSize(variant: string | undefined, theme: IxTheme): string {
  if (!variant) return theme.typography.fontSize.md;

  const sizeMap: Record<string, string> = {
    heading: theme.typography.fontSize.lg,
    body: theme.typography.fontSize.md,
    caption: theme.typography.fontSize.xs,
    label: theme.typography.fontSize.sm,
  };

  return sizeMap[variant] || theme.typography.fontSize.md;
}

/**
 * Parse markdown UI description to component tree
 */
function parseMarkdownToComponents(markdown: string): UIComponent[] {
  const components: UIComponent[] = [];
  const lines = markdown.split('\n').filter((line) => line.trim());

  const currentList: UIComponent | null = null;
  const listItems: UIComponent[] = [];

  for (const line of lines) {
    const trimmed = line.trim();

    // Heading
    if (trimmed.startsWith('#')) {
      const level = trimmed.match(/^#+/)?.[0].length || 1;
      const text = trimmed.replace(/^#+\s*/, '');
      components.push({
        id: generateId(),
        type: 'Heading',
        props: { level, children: text },
      });
    }
    // Paragraph
    else if (!trimmed.startsWith('-') && !trimmed.startsWith('*') && !trimmed.startsWith('>')) {
      components.push({
        id: generateId(),
        type: 'Text',
        props: { children: trimmed, variant: 'body' },
      });
    }
    // List item
    else if (trimmed.startsWith('-') || trimmed.startsWith('*')) {
      const text = trimmed.replace(/^[-*]\s*/, '');
      listItems.push({
        id: generateId(),
        type: 'Text',
        props: { children: text },
      });
    }
    // Blockquote
    else if (trimmed.startsWith('>')) {
      const text = trimmed.replace(/^>\s*/, '');
      components.push({
        id: generateId(),
        type: 'Text',
        props: { children: text, variant: 'caption', style: { fontStyle: 'italic', borderLeft: '3px solid #E5E7EB', paddingLeft: '12px' } },
      });
    }
  }

  // If we have list items, wrap them in a Stack
  if (listItems.length > 0) {
    components.push({
      id: generateId(),
      type: 'Stack',
      props: { direction: 'vertical', spacing: 4 },
      children: listItems,
    });
  }

  return components;
}

/**
 * LLM-to-IX Pipeline
 *
 * Main pipeline function that:
 * 1. Parses LLM output (JSON or markdown)
 * 2. Transforms to IX component tree
 * 3. Applies Allternit-IX styling
 * 4. Returns IxRenderTree
 */
export function llmToIxPipeline(options: LlmToIxPipelineOptions): LlmToIxPipelineResult {
  const startTime = Date.now();
  const pipelineOptions = options.options || {};

  // Parse input
  const parseStart = Date.now();
  const format = detectInputFormat(options.input);
  let convertResult: LLMToIXResult;

  // Handle markdown input
  if (format === 'natural-language' && typeof options.input === 'string') {
    // Check if it looks like markdown
    if (options.input.includes('#') || options.input.includes('- ') || options.input.includes('> ')) {
      const components = parseMarkdownToComponents(options.input);
      const ui: UIRoot = {
        version: pipelineOptions.version || '1.0.0',
        components,
        state: { variables: [] },
        actions: [],
      };
      convertResult = {
        ui,
        warnings: ['Parsed from markdown'],
        inferredState: [],
        confidence: 0.7,
      };
    } else {
      convertResult = convertFromNaturalLanguage(options.input, {
        version: pipelineOptions.version,
        generateIds: pipelineOptions.generateIds,
        validate: pipelineOptions.validate,
        inferState: pipelineOptions.inferState,
      });
    }
  } else {
    convertResult = convertLLMToIX(options.input, {
      version: pipelineOptions.version,
      generateIds: pipelineOptions.generateIds,
      validate: pipelineOptions.validate,
      inferState: pipelineOptions.inferState,
    });
  }

  const parseTime = Date.now() - parseStart;

  // Transform to render tree
  const transformStart = Date.now();
  const theme = pipelineOptions.theme ? getTheme(pipelineOptions.theme) : IX_THEMES.default;

  const styledComponents = pipelineOptions.applyStyling !== false
    ? applyStylingToTree(convertResult.ui.components, theme)
    : convertResult.ui.components.map((comp): IxRenderNode => ({
        id: comp.id,
        type: comp.type,
        props: comp.props,
        style: comp.style || {},
        className: comp.className,
        children: comp.children?.map((child): IxRenderNode => ({
          id: child.id,
          type: child.type,
          props: child.props,
          style: child.style || {},
          className: child.className,
          children: child.children?.map((c): IxRenderNode => ({
            id: c.id,
            type: c.type,
            props: c.props,
            style: c.style || {},
            className: c.className,
          })),
          bindings: child.bindings,
          events: child.events,
        })),
        bindings: comp.bindings,
        events: comp.events,
        condition: comp.condition,
        repeat: comp.repeat,
      }));

  const transformTime = Date.now() - transformStart;

  // Apply styling
  const styleStart = Date.now();
  const styles: Record<string, Record<string, string | number>> = {};

  if (pipelineOptions.applyStyling !== false) {
    // Generate CSS classes for components
    styledComponents.forEach((comp) => {
      styles[comp.id] = comp.style;
    });
  }

  const styleTime = Date.now() - styleStart;
  const totalTime = Date.now() - startTime;

  // Build render tree
  const tree: IxRenderTree = {
    version: convertResult.ui.version,
    components: styledComponents,
    state: convertResult.ui.state,
    actions: convertResult.ui.actions,
    theme: pipelineOptions.applyStyling !== false ? theme : undefined,
    styles: pipelineOptions.applyStyling !== false ? styles : undefined,
    metadata: {
      generatedAt: new Date().toISOString(),
    },
  };

  return {
    tree,
    uiRoot: convertResult.ui,
    warnings: convertResult.warnings,
    inferredState: convertResult.inferredState,
    confidence: convertResult.confidence,
    metrics: {
      parseTime,
      transformTime,
      styleTime,
      totalTime,
    },
  };
}

/**
 * LLM-to-IX Pipeline API
 */
export const llmToIX = {
  /**
   * Convert LLM output to IX format
   */
  convert: convertLLMToIX,

  /**
   * Detect input format
   */
  detectFormat: detectInputFormat,

  /**
   * Check if input is valid json-render
   */
  isJsonRender: (input: unknown) => jsonRenderAdapter.isValidSchema(input),

  /**
   * Quick convert from json-render
   */
  fromJsonRender: (schema: JsonRenderSchema, options?: LLMToIXOptions) =>
    convertFromJsonRender(schema, options || {}),

  /**
   * Quick convert from JSX-like
   */
  fromJSX: (jsx: string, options?: LLMToIXOptions) =>
    convertFromJSXLike(jsx, options || {}),
};
