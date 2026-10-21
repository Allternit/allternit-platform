/**
 * json-render Adapter Tests
 */

import { describe, it, expect } from 'vitest';
import {
  jsonRenderAdapter,
  convertToJsonRender,
  convertFromJsonRender,
  type JsonRenderSchema,
} from '../adapters/json-render';
import type { UIRoot, UIComponent } from '../types';

describe('json-render Adapter', () => {
  describe('convertToJsonRender', () => {
    it('should convert basic UIRoot to json-render', () => {
      const uiRoot: UIRoot = {
        version: '1.0.0',
        components: [
          {
            id: 'comp1',
            type: 'Text',
            props: { children: 'Hello World' },
          },
        ],
        state: { variables: [] },
        actions: [],
      };

      const jsonRender = convertToJsonRender(uiRoot);

      expect(jsonRender.version).toBe('1.0.0');
      expect(jsonRender.root.type).toBe('Fragment');
      expect(jsonRender.root.children).toHaveLength(1);
      expect(jsonRender.root.children![0].type).toBe('Text');
    });

    it('should convert nested components', () => {
      const uiRoot: UIRoot = {
        version: '1.0.0',
        components: [
          {
            id: 'container',
            type: 'Box',
            props: { padding: 4 },
            children: [
              {
                id: 'text',
                type: 'Text',
                props: { children: 'Nested' },
              },
            ],
          },
        ],
        state: { variables: [] },
        actions: [],
      };

      const jsonRender = convertToJsonRender(uiRoot);

      expect(jsonRender.root.children![0].children).toHaveLength(1);
      expect(jsonRender.root.children![0].children![0].type).toBe('Text');
    });

    it('should convert state bindings', () => {
      const uiRoot: UIRoot = {
        version: '1.0.0',
        components: [
          {
            id: 'input',
            type: 'Input',
            props: {},
            bindings: [
              {
                prop: 'value',
                statePath: 'form.name',
                direction: 'one-way',
              },
            ],
          },
        ],
        state: { variables: [] },
        actions: [],
      };

      const jsonRender = convertToJsonRender(uiRoot);
      const input = jsonRender.root.children![0];

      expect(input.props!.value).toEqual({ $state: 'form.name' });
    });

    it('should convert two-way bindings', () => {
      const uiRoot: UIRoot = {
        version: '1.0.0',
        components: [
          {
            id: 'input',
            type: 'Input',
            props: {},
            bindings: [
              {
                prop: 'value',
                statePath: 'form.email',
                direction: 'two-way',
              },
            ],
          },
        ],
        state: { variables: [] },
        actions: [],
      };

      const jsonRender = convertToJsonRender(uiRoot);
      const input = jsonRender.root.children![0];

      expect(input.props!.onValueChange).toEqual({
        $action: 'setState',
        path: 'form.email',
      });
    });

    it('should convert event handlers', () => {
      const uiRoot: UIRoot = {
        version: '1.0.0',
        components: [
          {
            id: 'btn',
            type: 'Button',
            props: { children: 'Submit' },
            events: [
              {
                event: 'onClick',
                action: 'submitForm',
                params: { formId: 'main' },
              },
            ],
          },
        ],
        state: { variables: [] },
        actions: [
          {
            id: 'submitForm',
            type: 'emitEvent',
            handler: { type: 'emitEvent', name: 'submit', payload: null },
          },
        ],
      };

      const jsonRender = convertToJsonRender(uiRoot);
      const btn = jsonRender.root.children![0];

      expect(btn.props!.onClick).toEqual({
        $action: 'submitForm',
        params: { formId: 'main' },
      });
    });

    it('should convert state definitions', () => {
      const uiRoot: UIRoot = {
        version: '1.0.0',
        components: [],
        state: {
          variables: [
            { path: 'count', type: 'number', default: 0 },
            { path: 'name', type: 'string' },
          ],
        },
        actions: [],
      };

      const jsonRender = convertToJsonRender(uiRoot);

      expect(jsonRender.state).toEqual({
        count: { type: 'number', default: 0 },
        name: { type: 'string' },
      });
    });

    it('should convert repeat bindings', () => {
      const uiRoot: UIRoot = {
        version: '1.0.0',
        components: [
          {
            id: 'list',
            type: 'List',
            props: {},
            repeat: {
              items: 'users',
              as: 'user',
              indexAs: 'index',
            },
          },
        ],
        state: { variables: [] },
        actions: [],
      };

      const jsonRender = convertToJsonRender(uiRoot);
      const list = jsonRender.root.children![0];

      expect(list.props!.$for).toEqual({
        items: 'users',
        as: 'user',
        index: 'index',
      });
    });

    it('should convert conditional rendering', () => {
      const uiRoot: UIRoot = {
        version: '1.0.0',
        components: [
          {
            id: 'modal',
            type: 'Modal',
            props: {},
            condition: { $path: 'showModal' },
          },
        ],
        state: { variables: [] },
        actions: [],
      };

      const jsonRender = convertToJsonRender(uiRoot);
      const modal = jsonRender.root.children![0];

      expect(modal.props!.$if).toEqual({ $path: 'showModal' });
    });
  });

  describe('convertFromJsonRender', () => {
    it('should convert json-render to UIRoot', () => {
      const schema: JsonRenderSchema = {
        version: '1.0.0',
        root: {
          type: 'Fragment',
          children: [
            {
              type: 'Text',
              props: { children: 'Hello' },
            },
          ],
        },
      };

      const uiRoot = convertFromJsonRender(schema);

      expect(uiRoot.version).toBe('1.0.0');
      expect(uiRoot.components).toHaveLength(1);
      expect(uiRoot.components[0].type).toBe('Text');
    });

    it('should convert state bindings from props', () => {
      const schema: JsonRenderSchema = {
        version: '1.0.0',
        root: {
          type: 'Input',
          props: {
            value: { $state: 'form.name' },
          },
        },
      };

      const uiRoot = convertFromJsonRender(schema);
      const input = uiRoot.components[0];

      expect(input.bindings).toContainEqual({
        prop: 'value',
        statePath: 'form.name',
        direction: 'one-way',
      });
    });

    it('should convert event handlers from props', () => {
      const schema: JsonRenderSchema = {
        version: '1.0.0',
        root: {
          type: 'Button',
          props: {
            onClick: { $action: 'handleClick', params: { id: 1 } },
          },
        },
      };

      const uiRoot = convertFromJsonRender(schema);
      const btn = uiRoot.components[0];

      expect(btn.events).toContainEqual({
        event: 'onClick',
        action: 'handleClick',
        params: { id: 1 },
      });
    });

    it('should convert for loops', () => {
      const schema: JsonRenderSchema = {
        version: '1.0.0',
        root: {
          type: 'List',
          props: {
            $for: {
              items: 'items',
              as: 'item',
              index: 'i',
            },
          },
        },
      };

      const uiRoot = convertFromJsonRender(schema);
      const list = uiRoot.components[0];

      expect(list.repeat).toEqual({
        items: 'items',
        as: 'item',
        indexAs: 'i',
      });
    });

    it('should convert state definitions', () => {
      const schema: JsonRenderSchema = {
        version: '1.0.0',
        root: { type: 'Box' },
        state: {
          count: { type: 'number', default: 0 },
          name: { type: 'string' },
        },
      };

      const uiRoot = convertFromJsonRender(schema);

      expect(uiRoot.state.variables).toContainEqual({
        path: 'count',
        type: 'number',
        default: 0,
      });
      expect(uiRoot.state.variables).toContainEqual({
        path: 'name',
        type: 'string',
      });
    });

    it('should convert events', () => {
      const schema: JsonRenderSchema = {
        version: '1.0.0',
        root: { type: 'Box' },
        events: {
          submitForm: { type: 'setState', target: 'submitted' },
          navigate: { type: 'navigate', target: '/home' },
        },
      };

      const uiRoot = convertFromJsonRender(schema);

      expect(uiRoot.actions).toHaveLength(2);
      const submitAction = uiRoot.actions.find((a) => a.id === 'submitForm');
      expect(submitAction?.handler.type).toBe('setState');
    });
  });

  describe('round-trip conversion', () => {
    it('should preserve structure through round-trip', () => {
      const original: UIRoot = {
        version: '1.0.0',
        components: [
          {
            id: 'comp1',
            type: 'Box',
            props: { padding: 4 },
            children: [
              {
                id: 'comp2',
                type: 'Text',
                props: { children: 'Hello' },
              },
            ],
          },
        ],
        state: {
          variables: [
            { path: 'count', type: 'number', default: 0 },
          ],
        },
        actions: [],
      };

      const jsonRender = convertToJsonRender(original);
      const roundTrip = convertFromJsonRender(jsonRender);

      expect(roundTrip.version).toBe(original.version);
      expect(roundTrip.components).toHaveLength(1);
      expect(roundTrip.components[0].type).toBe('Box');
      expect(roundTrip.components[0].children).toHaveLength(1);
    });
  });

  describe('jsonRenderAdapter', () => {
    it('should have correct metadata', () => {
      expect(jsonRenderAdapter.name).toBe('json-render');
      expect(jsonRenderAdapter.version).toBe('1.0.0');
    });

    it('should have component mappings', () => {
      expect(jsonRenderAdapter.components.Box).toBe('layout');
      expect(jsonRenderAdapter.components.Text).toBe('typography');
      expect(jsonRenderAdapter.components.Input).toBe('input');
      expect(jsonRenderAdapter.components.Card).toBe('display');
    });

    describe('isValidSchema', () => {
      it('should return true for valid schema', () => {
        const schema = {
          version: '1.0.0',
          root: { type: 'Box' },
        };
        expect(jsonRenderAdapter.isValidSchema(schema)).toBe(true);
      });

      it('should return false for invalid schema', () => {
        expect(jsonRenderAdapter.isValidSchema(null)).toBe(false);
        expect(jsonRenderAdapter.isValidSchema({})).toBe(false);
        expect(jsonRenderAdapter.isValidSchema({ version: '1.0.0' })).toBe(false);
      });
    });

    describe('isValidNode', () => {
      it('should return true for valid node', () => {
        expect(jsonRenderAdapter.isValidNode({ type: 'Box' })).toBe(true);
      });

      it('should return false for invalid node', () => {
        expect(jsonRenderAdapter.isValidNode(null)).toBe(false);
        expect(jsonRenderAdapter.isValidNode({})).toBe(false);
      });
    });
  });
});
