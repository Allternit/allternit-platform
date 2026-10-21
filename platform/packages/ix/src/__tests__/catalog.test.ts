/**
 * Component Catalog Tests
 */

import { describe, it, expect } from 'vitest';
import { ComponentCatalog, BUILT_INS, createDefaultCatalog } from '../catalog/registry';
import type { CatalogComponent } from '../catalog/registry';

describe('ComponentCatalog', () => {
  describe('registration', () => {
    it('should register components', () => {
      const catalog = new ComponentCatalog();
      const component: CatalogComponent = {
        name: 'TestComponent',
        version: '1.0.0',
        props: [{ name: 'value', type: 'string', required: true }],
        category: 'display',
      };

      catalog.register(component);
      const resolved = catalog.resolve('TestComponent');

      expect(resolved).toEqual(component);
    });

    it('should store multiple versions', () => {
      const catalog = new ComponentCatalog();
      
      catalog.register({
        name: 'Component',
        version: '1.0.0',
        props: [],
        category: 'display',
      });
      
      catalog.register({
        name: 'Component',
        version: '2.0.0',
        props: [{ name: 'newProp', type: 'string' }],
        category: 'display',
      });

      const v1 = catalog.resolve('Component', '1.0.0');
      const v2 = catalog.resolve('Component', '2.0.0');
      const latest = catalog.resolve('Component');

      expect(v1?.props).toHaveLength(0);
      expect(v2?.props).toHaveLength(1);
      expect(latest?.version).toBe('2.0.0');
    });
  });

  describe('semantic versioning', () => {
    it('should resolve exact versions', () => {
      const catalog = new ComponentCatalog();
      
      catalog.register({
        name: 'Comp',
        version: '1.2.3',
        props: [],
        category: 'display',
      });

      expect(catalog.resolve('Comp', '1.2.3')?.version).toBe('1.2.3');
    });

    it('should resolve latest with caret range', () => {
      const catalog = new ComponentCatalog();
      
      catalog.register({
        name: 'Comp',
        version: '1.0.0',
        props: [],
        category: 'display',
      });
      
      catalog.register({
        name: 'Comp',
        version: '1.2.0',
        props: [],
        category: 'display',
      });
      
      catalog.register({
        name: 'Comp',
        version: '2.0.0',
        props: [],
        category: 'display',
      });

      expect(catalog.resolve('Comp', '^1.0.0')?.version).toBe('1.2.0');
    });

    it('should resolve tilde range', () => {
      const catalog = new ComponentCatalog();
      
      catalog.register({
        name: 'Comp',
        version: '1.0.0',
        props: [],
        category: 'display',
      });
      
      catalog.register({
        name: 'Comp',
        version: '1.0.5',
        props: [],
        category: 'display',
      });
      
      catalog.register({
        name: 'Comp',
        version: '1.1.0',
        props: [],
        category: 'display',
      });

      expect(catalog.resolve('Comp', '~1.0.0')?.version).toBe('1.0.5');
    });
  });

  describe('validation', () => {
    it('should validate component props', () => {
      const catalog = new ComponentCatalog();
      
      catalog.register({
        name: 'Button',
        version: '1.0.0',
        props: [
          { name: 'label', type: 'string', required: true },
          { name: 'disabled', type: 'boolean', default: false },
        ],
        category: 'input',
      });

      expect(catalog.validate('Button', { label: 'Click me' })).toBe(true);
      expect(catalog.validate('Button', {})).toBe(false); // Missing required prop
    });

    it('should validate prop types', () => {
      const catalog = new ComponentCatalog();
      
      catalog.register({
        name: 'Counter',
        version: '1.0.0',
        props: [
          { name: 'count', type: 'number' },
          { name: 'enabled', type: 'boolean' },
        ],
        category: 'display',
      });

      expect(catalog.validate('Counter', { count: 42 })).toBe(true);
      expect(catalog.validate('Counter', { count: '42' })).toBe(false);
      expect(catalog.validate('Counter', { enabled: true })).toBe(true);
      expect(catalog.validate('Counter', { enabled: 'true' })).toBe(false);
    });

    it('should validate enum values', () => {
      const catalog = new ComponentCatalog();
      
      catalog.register({
        name: 'Status',
        version: '1.0.0',
        props: [
          {
            name: 'variant',
            type: 'string',
            enum: ['success', 'error', 'warning'],
          },
        ],
        category: 'display',
      });

      expect(catalog.validate('Status', { variant: 'success' })).toBe(true);
      expect(catalog.validate('Status', { variant: 'invalid' })).toBe(false);
    });
  });

  describe('discovery', () => {
    it('should list all components', () => {
      const catalog = new ComponentCatalog();
      
      catalog.register({
        name: 'A',
        version: '1.0.0',
        props: [],
        category: 'layout',
      });
      
      catalog.register({
        name: 'B',
        version: '1.0.0',
        props: [],
        category: 'display',
      });

      const all = catalog.list();
      expect(all).toHaveLength(2);
      expect(all.map((c) => c.name)).toContain('A');
      expect(all.map((c) => c.name)).toContain('B');
    });

    it('should filter by category', () => {
      const catalog = new ComponentCatalog();
      
      catalog.register({
        name: 'Box',
        version: '1.0.0',
        props: [],
        category: 'layout',
      });
      
      catalog.register({
        name: 'Text',
        version: '1.0.0',
        props: [],
        category: 'typography',
      });

      const layout = catalog.list('layout');
      expect(layout).toHaveLength(1);
      expect(layout[0].name).toBe('Box');
    });
  });

  describe('BUILT_INS', () => {
    it('should have all standard components', () => {
      const expected = [
        'Box', 'Stack', 'Text', 'Heading', 'Paragraph',
        'Button', 'Input', 'TextArea', 'Select', 'Image',
        'Icon', 'Badge', 'Card', 'List', 'Table',
      ];

      expected.forEach((name) => {
        expect(BUILT_INS).toHaveProperty(name);
      });
    });

    it('should have correct categories', () => {
      expect(BUILT_INS.Box.category).toBe('layout');
      expect(BUILT_INS.Text.category).toBe('typography');
      expect(BUILT_INS.Button.category).toBe('input');
      expect(BUILT_INS.Card.category).toBe('display');
    });
  });

  describe('createDefaultCatalog', () => {
    it('should create catalog with built-ins', () => {
      const catalog = createDefaultCatalog();
      
      expect(catalog.resolve('Box')).toBeDefined();
      expect(catalog.resolve('Text')).toBeDefined();
      expect(catalog.resolve('Button')).toBeDefined();
    });

    it('should have 15 built-in components', () => {
      const catalog = createDefaultCatalog();
      const all = catalog.list();
      
      expect(all).toHaveLength(15);
    });
  });
});
