/**
 * Vue Renderer for Allternit-IX
 * 
 * Renders Allternit-IX UI IR to Vue 3 components.
 */

import type { UIRoot, UIComponent } from '../types';
import type { StateStore } from '../state/store';
import type { ComponentCatalog } from '../catalog/registry';
import { createDefaultCatalog } from '../catalog/registry';
import { evaluateExpression } from '../types';

// Vue imports - these are type-only for compilation
import type { DefineComponent, VNode, RendererNode, RendererElement } from 'vue';

export interface VueRendererConfig {
  /** Component catalog */
  catalog?: ComponentCatalog;
  /** Initial state store */
  stateStore?: StateStore;
  /** Component mapping (type -> Vue component) */
  components?: Record<string, DefineComponent>;
  /** Event handler */
  onEvent?: (name: string, payload: unknown) => void;
  /** Error handler */
  onError?: (error: Error, componentId?: string) => void;
}

export interface VueRenderer {
  /** Generate component definition for Vue */
  generateComponent(root: UIRoot): VueComponentDefinition;
}

export interface VueComponentDefinition {
  /** Component name */
  name: string;
  /** Props definition */
  props: Record<string, unknown>;
  /** Setup function code */
  setup: string;
  /** Template */
  template: string;
  /** Styles */
  styles?: string;
}

/**
 * Create Vue renderer
 */
export function createVueRenderer(config: VueRendererConfig): VueRenderer {
  const catalog = config.catalog ?? createDefaultCatalog();

  /**
   * Generate component definition
   */
  function generateComponent(root: UIRoot): VueComponentDefinition {
    const name = 'AllternitIXRoot';
    
    // Generate props from state variables
    const props: Record<string, unknown> = {};
    root.state.variables.forEach((variable) => {
      props[sanitizeName(variable.path)] = {
        type: getVueType(variable.type),
        default: variable.default,
      };
    });

    // Generate setup function
    const setupLines: string[] = [
      '// Allternit-IX Generated Component',
      "import { ref, computed, watch } from 'vue';",
      '',
      '// State',
    ];

    // State refs
    root.state.variables.forEach((variable) => {
      const varName = sanitizeName(variable.path);
      setupLines.push(`const ${varName} = ref(props.${varName});`);
    });

    // Computed values
    if (root.state.computed) {
      setupLines.push('');
      setupLines.push('// Computed values');
      root.state.computed.forEach((computed) => {
        const deps = computed.deps.map(sanitizeName).join(', ');
        const expr = generateExpression(computed.compute);
        setupLines.push(`const ${computed.name} = computed(() => {`);
        setupLines.push(`  return ${expr};`);
        setupLines.push('});');
      });
    }

    // Event handlers
    setupLines.push('');
    setupLines.push('// Event handlers');
    root.actions.forEach((action) => {
      setupLines.push(`const ${action.id} = (payload) => {`);
      setupLines.push(`  emit('${action.id}', payload);`);
      setupLines.push('};');
    });

    // Expose
    setupLines.push('');
    setupLines.push('return {');
    root.state.variables.forEach((variable) => {
      setupLines.push(`  ${sanitizeName(variable.path)},`);
    });
    root.state.computed?.forEach((computed) => {
      setupLines.push(`  ${computed.name},`);
    });
    root.actions.forEach((action) => {
      setupLines.push(`  ${action.id},`);
    });
    setupLines.push('};');

    // Generate template
    const template = root.components.map((c) => generateTemplate(c)).join('\n');

    return {
      name,
      props,
      setup: setupLines.join('\n'),
      template,
      styles: root.css,
    };
  }

  /**
   * Generate template for component
   */
  function generateTemplate(component: UIComponent, depth = 0): string {
    const indent = '  '.repeat(depth);
    const tag = getComponentTag(component.type);

    // Handle condition
    if (component.condition) {
      const condition = generateExpression(component.condition);
      const inner = generateSingleTemplate(component, depth + 1);
      return `${indent}<template v-if="${condition}">\n${inner}\n${indent}</template>`;
    }

    // Handle repeat
    if (component.repeat) {
      return generateRepeatTemplate(component, depth);
    }

    return generateSingleTemplate(component, depth);
  }

  /**
   * Generate single component template
   */
  function generateSingleTemplate(component: UIComponent, depth: number): string {
    const indent = '  '.repeat(depth);
    const tag = getComponentTag(component.type);
    const props = generateProps(component);
    const events = generateEvents(component);

    const propString = [...props, ...events].join(' ');

    if (component.children && component.children.length > 0) {
      const children = component.children.map((c) => generateTemplate(c, depth + 1)).join('\n');
      return `${indent}<${tag}${propString ? ' ' + propString : ''}>\n${children}\n${indent}</${tag}>`;
    }

    // Self-closing or with text
    const textContent = component.props?.children ? escapeHtml(String(component.props.children)) : '';
    if (textContent) {
      return `${indent}<${tag}${propString ? ' ' + propString : ''}>${textContent}</${tag}>`;
    }

    return `${indent}<${tag}${propString ? ' ' + propString : ''} />`;
  }

  /**
   * Generate repeat template
   */
  function generateRepeatTemplate(component: UIComponent, depth: number): string {
    const indent = '  '.repeat(depth);
    const repeat = component.repeat!;
    const tag = getComponentTag(component.type);
    const props = generateProps(component);
    const events = generateEvents(component);
    const propString = [...props, ...events].join(' ');

    const itemsVar = sanitizeName(repeat.items);
    const asVar = sanitizeName(repeat.as);
    const indexVar = repeat.indexAs ? sanitizeName(repeat.indexAs) : 'index';

    let eachBlock = `${indent}<${tag}`;
    eachBlock += ` v-for="(${asVar}, ${indexVar}) in ${itemsVar}"`;
    eachBlock += ` :key="${indexVar}"`;
    if (propString) {
      eachBlock += ' ' + propString;
    }

    if (component.children && component.children.length > 0) {
      const children = component.children.map((c) => generateTemplate(c, depth + 1)).join('\n');
      eachBlock += `>\n${children}\n${indent}</${tag}>`;
    } else {
      eachBlock += ' />';
    }

    return eachBlock;
  }

  /**
   * Generate props string
   */
  function generateProps(component: UIComponent): string[] {
    const props: string[] = [];

    // Regular props
    Object.entries(component.props || {}).forEach(([key, value]) => {
      if (key !== 'children') {
        if (typeof value === 'string') {
          props.push(`${key}="${escapeHtml(value)}"`);
        } else if (typeof value === 'boolean') {
          if (value) props.push(key);
        } else if (typeof value === 'number') {
          props.push(`:${key}="${value}"`);
        } else {
          props.push(`:${key}="${JSON.stringify(value).replace(/"/g, '&quot;')}"`);
        }
      }
    });

    // Bindings
    component.bindings?.forEach((binding) => {
      const stateVar = sanitizeName(binding.statePath);
      if (binding.direction === 'two-way') {
        props.push(`v-model:${binding.prop}="${stateVar}"`);
      } else {
        props.push(`:${binding.prop}="${stateVar}"`);
      }
    });

    return props;
  }

  /**
   * Generate event handlers
   */
  function generateEvents(component: UIComponent): string[] {
    const events: string[] = [];

    component.events?.forEach((event) => {
      const vueEvent = getVueEventName(event.event);
      events.push(`@${vueEvent}="${event.action}($event)"`);
    });

    return events;
  }

  /**
   * Get component tag
   */
  function getComponentTag(type: string): string {
    const tagMap: Record<string, string> = {
      Box: 'div',
      Stack: 'div',
      Text: 'span',
      Heading: 'h1',
      Paragraph: 'p',
      Button: 'button',
      Input: 'input',
      TextArea: 'textarea',
      Select: 'select',
      Card: 'div',
      Image: 'img',
      Icon: 'span',
      Badge: 'span',
      List: 'ul',
      Table: 'table',
    };

    return tagMap[type] || type.toLowerCase();
  }

  /**
   * Get Vue type from schema type
   */
  function getVueType(type: string): string {
    const typeMap: Record<string, string> = {
      string: 'String',
      number: 'Number',
      boolean: 'Boolean',
      array: 'Array',
      object: 'Object',
      any: 'null',
    };
    return typeMap[type] || 'null';
  }

  /**
   * Convert event name to Vue format
   */
  function getVueEventName(event: string): string {
    if (event.startsWith('on')) {
      return event.slice(2).toLowerCase();
    }
    return event.toLowerCase();
  }

  /**
   * Generate expression
   */
  function generateExpression(expr: unknown): string {
    if (expr === null) return 'null';
    if (typeof expr === 'string') return sanitizeName(expr);
    if (typeof expr === 'number') return String(expr);
    if (typeof expr === 'boolean') return String(expr);

    if (typeof expr === 'object') {
      if ('$path' in (expr as object)) {
        return sanitizeName((expr as { $path: string }).$path);
      }
      if ('$expr' in (expr as object)) {
        return (expr as { $expr: string }).$expr;
      }
      if ('$cond' in (expr as object)) {
        const [cond, trueVal, falseVal] = (expr as { $cond: [unknown, unknown, unknown] }).$cond;
        return `${generateExpression(cond)} ? ${generateExpression(trueVal)} : ${generateExpression(falseVal)}`;
      }
      if ('$and' in (expr as object)) {
        return (expr as { $and: unknown[] }).$and.map(generateExpression).join(' && ');
      }
      if ('$or' in (expr as object)) {
        return (expr as { $or: unknown[] }).$or.map(generateExpression).join(' || ');
      }
      if ('$eq' in (expr as object)) {
        const [a, b] = (expr as { $eq: [unknown, unknown] }).$eq;
        return `${generateExpression(a)} === ${generateExpression(b)}`;
      }
    }

    return String(expr);
  }

  /**
   * Sanitize variable name
   */
  function sanitizeName(path: string): string {
    return path.replace(/\./g, '_').replace(/[^a-zA-Z0-9_]/g, '');
  }

  /**
   * Escape HTML
   */
  function escapeHtml(text: string): string {
    return text
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#039;');
  }

  return {
    generateComponent,
  };
}

/**
 * Compile Allternit-IX UI to Vue component definition
 */
export function compileToVue(root: UIRoot, config: VueRendererConfig = {}): VueComponentDefinition {
  const renderer = createVueRenderer(config);
  return renderer.generateComponent(root);
}
