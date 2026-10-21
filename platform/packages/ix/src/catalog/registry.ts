/**
 * Component Catalog Registry
 * 
 * Versioned component registry with semantic versioning support.
 */

export interface PropDefinition {
  name: string;
  type: 'string' | 'number' | 'boolean' | 'array' | 'object' | 'function' | 'any';
  required?: boolean;
  default?: unknown;
  enum?: unknown[];
  description?: string;
}

export interface CatalogComponent {
  name: string;
  version: string;
  props: PropDefinition[];
  category: 'layout' | 'typography' | 'input' | 'display';
  description?: string;
  acceptChildren?: boolean;
  allowedChildren?: string[];
}

/**
 * Component Catalog
 */
export class ComponentCatalog {
  private components = new Map<string, CatalogComponent>();

  /**
   * Register a component
   */
  register(component: CatalogComponent): void {
    const key = `${component.name}@${component.version}`;
    this.components.set(key, component);
  }

  /**
   * Resolve component by name and optional version
   */
  resolve(name: string, version?: string): CatalogComponent | undefined {
    if (version) {
      // Exact version
      if (!version.includes('^') && !version.includes('~') && !version.includes('*')) {
        return this.components.get(`${name}@${version}`);
      }

      // Semantic version range
      return this.resolveVersion(name, version);
    }

    // Get latest version
    return this.getLatest(name);
  }

  /**
   * Get latest version of component
   */
  private getLatest(name: string): CatalogComponent | undefined {
    const versions: CatalogComponent[] = [];
    
    for (const [key, component] of this.components) {
      if (key.startsWith(`${name}@`)) {
        versions.push(component);
      }
    }

    if (versions.length === 0) return undefined;

    // Sort by version descending
    versions.sort((a, b) => compareVersions(b.version, a.version));
    return versions[0];
  }

  /**
   * Resolve version with semver range
   */
  private resolveVersion(name: string, range: string): CatalogComponent | undefined {
    const versions: CatalogComponent[] = [];
    
    for (const [key, component] of this.components) {
      if (key.startsWith(`${name}@`)) {
        versions.push(component);
      }
    }

    if (versions.length === 0) return undefined;

    // Sort by version descending
    versions.sort((a, b) => compareVersions(b.version, a.version));

    // Parse range
    if (range.startsWith('^')) {
      // ^1.2.3 matches >=1.2.3 <2.0.0
      const baseVersion = range.slice(1);
      const [major] = baseVersion.split('.').map(Number);
      
      return versions.find((v) => {
        const [vMajor] = v.version.split('.').map(Number);
        return vMajor === major && compareVersions(v.version, baseVersion) >= 0;
      });
    }

    if (range.startsWith('~')) {
      // ~1.2.3 matches >=1.2.3 <1.3.0
      const baseVersion = range.slice(1);
      const [major, minor] = baseVersion.split('.').map(Number);
      
      return versions.find((v) => {
        const [vMajor, vMinor] = v.version.split('.').map(Number);
        return vMajor === major && vMinor === minor && compareVersions(v.version, baseVersion) >= 0;
      });
    }

    // Default: exact match or latest
    return versions[0];
  }

  /**
   * Validate props against component schema
   */
  validate(name: string, props: Record<string, unknown>, version?: string): boolean {
    const component = this.resolve(name, version);
    if (!component) return false;

    // Check required props
    for (const prop of component.props) {
      if (prop.required && !(prop.name in props)) {
        return false;
      }
    }

    // Validate prop types
    for (const [key, value] of Object.entries(props)) {
      const propDef = component.props.find((p) => p.name === key);
      if (!propDef) continue; // Allow unknown props

      if (!validateType(value, propDef.type)) {
        return false;
      }

      if (propDef.enum && !propDef.enum.includes(value)) {
        return false;
      }
    }

    return true;
  }

  /**
   * List all components
   */
  list(category?: string): CatalogComponent[] {
    const all = Array.from(this.components.values());
    
    if (category) {
      return all.filter((c) => c.category === category);
    }

    return all;
  }

  /**
   * Get available versions for component
   */
  getVersions(name: string): string[] {
    const versions: string[] = [];
    
    for (const key of this.components.keys()) {
      if (key.startsWith(`${name}@`)) {
        versions.push(key.slice(name.length + 1));
      }
    }

    return versions.sort((a, b) => compareVersions(b, a));
  }
}

/**
 * Compare semantic versions
 * Returns: positive if a > b, negative if a < b, 0 if equal
 */
function compareVersions(a: string, b: string): number {
  const aParts = a.split('.').map(Number);
  const bParts = b.split('.').map(Number);

  for (let i = 0; i < Math.max(aParts.length, bParts.length); i++) {
    const aPart = aParts[i] || 0;
    const bPart = bParts[i] || 0;

    if (aPart > bPart) return 1;
    if (aPart < bPart) return -1;
  }

  return 0;
}

/**
 * Validate value against type
 */
function validateType(value: unknown, type: PropDefinition['type']): boolean {
  switch (type) {
    case 'string':
      return typeof value === 'string';
    case 'number':
      return typeof value === 'number' && !isNaN(value);
    case 'boolean':
      return typeof value === 'boolean';
    case 'array':
      return Array.isArray(value);
    case 'object':
      return typeof value === 'object' && value !== null && !Array.isArray(value);
    case 'function':
      return typeof value === 'function';
    case 'any':
      return true;
    default:
      return false;
  }
}

/**
 * Built-in component definitions
 */
export const BUILT_INS: Record<string, CatalogComponent> = {
  // Layout
  Box: {
    name: 'Box',
    version: '1.0.0',
    category: 'layout',
    props: [
      { name: 'padding', type: 'number' },
      { name: 'margin', type: 'number' },
      { name: 'width', type: 'any' },
      { name: 'height', type: 'any' },
      { name: 'display', type: 'string', enum: ['block', 'flex', 'grid', 'none'] },
      { name: 'flexDirection', type: 'string', enum: ['row', 'column'] },
      { name: 'justifyContent', type: 'string' },
      { name: 'alignItems', type: 'string' },
      { name: 'gap', type: 'number' },
      { name: 'backgroundColor', type: 'string' },
      { name: 'borderRadius', type: 'number' },
    ],
    acceptChildren: true,
  },
  Stack: {
    name: 'Stack',
    version: '1.0.0',
    category: 'layout',
    props: [
      { name: 'direction', type: 'string', enum: ['horizontal', 'vertical'], default: 'vertical' },
      { name: 'spacing', type: 'number', default: 0 },
      { name: 'align', type: 'string', enum: ['start', 'center', 'end', 'stretch'] },
      { name: 'justify', type: 'string', enum: ['start', 'center', 'end', 'between', 'around'] },
    ],
    acceptChildren: true,
  },
  
  // Typography
  Text: {
    name: 'Text',
    version: '1.0.0',
    category: 'typography',
    props: [
      { name: 'children', type: 'any', required: true },
      { name: 'variant', type: 'string', enum: ['body', 'heading', 'caption', 'label'] },
      { name: 'size', type: 'string', enum: ['xs', 'sm', 'md', 'lg', 'xl'] },
      { name: 'weight', type: 'string', enum: ['normal', 'medium', 'semibold', 'bold'] },
      { name: 'color', type: 'string' },
      { name: 'align', type: 'string', enum: ['left', 'center', 'right'] },
      { name: 'truncate', type: 'boolean', default: false },
    ],
    acceptChildren: false,
  },
  Heading: {
    name: 'Heading',
    version: '1.0.0',
    category: 'typography',
    props: [
      { name: 'children', type: 'any', required: true },
      { name: 'level', type: 'number', enum: [1, 2, 3, 4, 5, 6], default: 1 },
      { name: 'color', type: 'string' },
    ],
    acceptChildren: false,
  },
  Paragraph: {
    name: 'Paragraph',
    version: '1.0.0',
    category: 'typography',
    props: [
      { name: 'children', type: 'any', required: true },
      { name: 'size', type: 'string', enum: ['sm', 'md', 'lg'] },
      { name: 'color', type: 'string' },
    ],
    acceptChildren: false,
  },

  // Input
  Button: {
    name: 'Button',
    version: '1.0.0',
    category: 'input',
    props: [
      { name: 'children', type: 'any', required: true },
      { name: 'variant', type: 'string', enum: ['primary', 'secondary', 'ghost', 'danger'], default: 'primary' },
      { name: 'size', type: 'string', enum: ['sm', 'md', 'lg'], default: 'md' },
      { name: 'disabled', type: 'boolean', default: false },
      { name: 'loading', type: 'boolean', default: false },
      { name: 'onClick', type: 'function' },
    ],
    acceptChildren: false,
  },
  Input: {
    name: 'Input',
    version: '1.0.0',
    category: 'input',
    props: [
      { name: 'value', type: 'string' },
      { name: 'placeholder', type: 'string' },
      { name: 'type', type: 'string', enum: ['text', 'password', 'email', 'number', 'tel'], default: 'text' },
      { name: 'disabled', type: 'boolean', default: false },
      { name: 'required', type: 'boolean', default: false },
      { name: 'onChange', type: 'function' },
      { name: 'onBlur', type: 'function' },
      { name: 'onFocus', type: 'function' },
    ],
    acceptChildren: false,
  },
  TextArea: {
    name: 'TextArea',
    version: '1.0.0',
    category: 'input',
    props: [
      { name: 'value', type: 'string' },
      { name: 'placeholder', type: 'string' },
      { name: 'rows', type: 'number', default: 3 },
      { name: 'disabled', type: 'boolean', default: false },
      { name: 'onChange', type: 'function' },
    ],
    acceptChildren: false,
  },
  Select: {
    name: 'Select',
    version: '1.0.0',
    category: 'input',
    props: [
      { name: 'value', type: 'any' },
      { name: 'options', type: 'array', required: true },
      { name: 'placeholder', type: 'string' },
      { name: 'disabled', type: 'boolean', default: false },
      { name: 'onChange', type: 'function' },
    ],
    acceptChildren: false,
  },

  // Display
  Image: {
    name: 'Image',
    version: '1.0.0',
    category: 'display',
    props: [
      { name: 'src', type: 'string', required: true },
      { name: 'alt', type: 'string', default: '' },
      { name: 'width', type: 'any' },
      { name: 'height', type: 'any' },
      { name: 'objectFit', type: 'string', enum: ['cover', 'contain', 'fill', 'none'] },
    ],
    acceptChildren: false,
  },
  Icon: {
    name: 'Icon',
    version: '1.0.0',
    category: 'display',
    props: [
      { name: 'name', type: 'string', required: true },
      { name: 'size', type: 'number', default: 16 },
      { name: 'color', type: 'string' },
    ],
    acceptChildren: false,
  },
  Badge: {
    name: 'Badge',
    version: '1.0.0',
    category: 'display',
    props: [
      { name: 'children', type: 'any', required: true },
      { name: 'variant', type: 'string', enum: ['default', 'success', 'warning', 'error', 'info'], default: 'default' },
      { name: 'size', type: 'string', enum: ['sm', 'md'], default: 'md' },
    ],
    acceptChildren: false,
  },
  Card: {
    name: 'Card',
    version: '1.0.0',
    category: 'display',
    props: [
      { name: 'padding', type: 'number', default: 16 },
      { name: 'elevation', type: 'number', enum: [0, 1, 2, 3], default: 1 },
      { name: 'borderRadius', type: 'number', default: 8 },
    ],
    acceptChildren: true,
  },
  List: {
    name: 'List',
    version: '1.0.0',
    category: 'display',
    props: [
      { name: 'items', type: 'array', required: true },
      { name: 'renderItem', type: 'function', required: true },
      { name: 'keyExtractor', type: 'function' },
      { name: 'emptyText', type: 'string' },
    ],
    acceptChildren: false,
  },
  Table: {
    name: 'Table',
    version: '1.0.0',
    category: 'display',
    props: [
      { name: 'data', type: 'array', required: true },
      { name: 'columns', type: 'array', required: true },
      { name: 'onRowClick', type: 'function' },
      { name: 'emptyText', type: 'string' },
    ],
    acceptChildren: false,
  },
};

/**
 * Create default catalog with built-ins
 */
export function createDefaultCatalog(): ComponentCatalog {
  const catalog = new ComponentCatalog();
  
  Object.values(BUILT_INS).forEach((component) => {
    catalog.register(component);
  });

  return catalog;
}
