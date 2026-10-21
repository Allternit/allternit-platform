/**
 * LLM-to-IX Pipeline Tests
 */

import { describe, it, expect } from 'vitest';
import { convertLLMToIX, llmToIX } from '../pipeline/llm-to-ix';
import type { JsonRenderSchema } from '../adapters/json-render';

describe('LLM-to-IX Pipeline', () => {
  describe('format detection', () => {
    it('should detect json-render format', () => {
      const jsonRender = {
        version: '1.0.0',
        root: { type: 'Box' },
      };
      expect(llmToIX.detectFormat(jsonRender)).toBe('json-render');
    });

    it('should detect JSX-like format', () => {
      const jsx = '<Box><Text>Hello</Text></Box>';
      expect(llmToIX.detectFormat(jsx)).toBe('jsx-like');
    });

    it('should detect natural language', () => {
      const nl = 'Create a form with a name input and submit button';
      expect(llmToIX.detectFormat(nl)).toBe('natural-language');
    });

    it('should detect component tree', () => {
      const tree = {
        components: [{ type: 'Button', props: {} }],
      };
      expect(llmToIX.detectFormat(tree)).toBe('component-tree');
    });

    it('should return unknown for invalid input', () => {
      expect(llmToIX.detectFormat(null)).toBe('unknown');
      expect(llmToIX.detectFormat(123)).toBe('unknown');
    });
  });

  describe('json-render conversion', () => {
    it('should convert json-render to IX', () => {
      const jsonRender: JsonRenderSchema = {
        version: '1.0.0',
        root: {
          type: 'Fragment',
          children: [
            { type: 'Text', props: { children: 'Hello' } },
            { type: 'Button', props: { children: 'Click' } },
          ],
        },
      };

      const result = llmToIX.fromJsonRender(jsonRender);

      expect(result.confidence).toBe(0.95);
      expect(result.ui.components).toHaveLength(2);
      expect(result.ui.components[0].type).toBe('Text');
      expect(result.ui.components[1].type).toBe('Button');
    });

    it('should handle state definitions', () => {
      const jsonRender: JsonRenderSchema = {
        version: '1.0.0',
        root: { type: 'Box' },
        state: {
          count: { type: 'number', default: 0 },
          name: { type: 'string' },
        },
      };

      const result = llmToIX.fromJsonRender(jsonRender);

      expect(result.ui.state.variables).toHaveLength(2);
      expect(result.ui.state.variables[0].path).toBe('count');
      expect(result.ui.state.variables[0].type).toBe('number');
    });

    it('should generate IDs when requested', () => {
      const jsonRender: JsonRenderSchema = {
        version: '1.0.0',
        root: {
          type: 'Fragment',
          children: [{ type: 'Text' }],
        },
      };

      const result = llmToIX.fromJsonRender(jsonRender, { generateIds: true });

      expect(result.ui.components[0].id).toBeDefined();
    });
  });

  describe('JSX-like conversion', () => {
    it('should convert JSX to IX', () => {
      const jsx = '<Box><Text>Hello World</Text></Box>';
      const result = llmToIX.fromJSX(jsx);

      expect(result.confidence).toBe(0.85);
      expect(result.ui.components).toHaveLength(1);
      expect(result.ui.components[0].type).toBe('Box');
      expect(result.ui.components[0].children?.[0].type).toBe('Text');
    });

    it('should parse props', () => {
      const jsx = '<Button variant="primary" disabled>Submit</Button>';
      const result = llmToIX.fromJSX(jsx);

      const button = result.ui.components[0];
      expect(button.props.variant).toBe('primary');
      expect(button.props.disabled).toBe(true);
      expect(button.props.children).toBe('Submit');
    });

    it('should parse bindings', () => {
      const jsx = '<Input $value={user.name} />';
      const result = llmToIX.fromJSX(jsx);

      const input = result.ui.components[0];
      expect(input.bindings).toHaveLength(1);
      expect(input.bindings?.[0].prop).toBe('value');
      expect(input.bindings?.[0].statePath).toBe('user.name');
    });

    it('should parse event handlers', () => {
      const jsx = '<Button onClick="handleClick">Click</Button>';
      const result = llmToIX.fromJSX(jsx);

      const button = result.ui.components[0];
      expect(button.events).toHaveLength(1);
      expect(button.events?.[0].event).toBe('onClick');
      expect(button.events?.[0].action).toBe('handleClick');
    });

    it('should handle self-closing tags', () => {
      const jsx = '<Box><Input /><Input /></Box>';
      const result = llmToIX.fromJSX(jsx);

      expect(result.ui.components[0].children).toHaveLength(2);
    });

    it('should handle nested components', () => {
      const jsx = `
        <Card>
          <Heading level={2}>Title</Heading>
          <Text>Description</Text>
          <Stack direction="horizontal">
            <Button>Save</Button>
            <Button>Cancel</Button>
          </Stack>
        </Card>
      `;
      const result = llmToIX.fromJSX(jsx);

      expect(result.ui.components[0].children).toHaveLength(3);
    });
  });

  describe('component tree conversion', () => {
    it('should convert flat component tree', () => {
      const tree = {
        components: [
          { type: 'Text', props: { children: 'Hello' } },
          { type: 'Button', props: { children: 'Click' } },
        ],
      };

      const result = convertLLMToIX(tree);

      expect(result.ui.components).toHaveLength(2);
    });

    it('should convert single component', () => {
      const tree = {
        type: 'Box',
        props: { padding: 4 },
        children: [{ type: 'Text', props: { children: 'Nested' } }],
      };

      const result = convertLLMToIX(tree);

      expect(result.ui.components).toHaveLength(1);
      expect(result.ui.components[0].children).toHaveLength(1);
    });
  });

  describe('generic conversion', () => {
    it('should handle arrays', () => {
      const input = [
        { type: 'Text', props: {} },
        { type: 'Button', props: {} },
      ];

      const result = convertLLMToIX(input);

      expect(result.ui.components).toHaveLength(2);
      expect(result.warnings).toContain('Using generic conversion');
    });

    it('should handle objects', () => {
      const input = { type: 'Card', props: { elevation: 2 } };

      const result = convertLLMToIX(input);

      expect(result.ui.components).toHaveLength(1);
      expect(result.ui.components[0].type).toBe('Card');
    });
  });

  describe('state inference', () => {
    it('should infer state from bindings', () => {
      const jsx = `
        <Box>
          <Input $value={form.email} />
          <Input $value={form.password} />
        </Box>
      `;
      const result = llmToIX.fromJSX(jsx, { inferState: true });

      expect(result.inferredState).toHaveLength(2);
      expect(result.inferredState.map((s) => s.path)).toContain('form.email');
      expect(result.inferredState.map((s) => s.path)).toContain('form.password');
    });

    it('should not duplicate existing state', () => {
      const tree = {
        components: [
          {
            type: 'Input',
            bindings: [{ prop: 'value', statePath: 'name', direction: 'one-way' }],
          },
        ],
        state: {
          variables: [{ path: 'name', type: 'string' }],
        },
      };

      const result = convertLLMToIX(tree, { inferState: true });

      expect(result.inferredState).toHaveLength(0);
    });
  });

  describe('error handling', () => {
    it('should handle invalid JSX gracefully', () => {
      const invalidJsx = '<Unclosed>';
      const result = llmToIX.fromJSX(invalidJsx);

      expect(result.confidence).toBeLessThan(0.5);
      expect(result.warnings.length).toBeGreaterThan(0);
    });

    it('should return empty UI for natural language', () => {
      const nl = 'Create a beautiful form with inputs and buttons for submission';
      const result = convertLLMToIX(nl);

      expect(result.confidence).toBe(0.1);
      expect(result.warnings).toContain('Natural language conversion requires LLM assistance');
    });
  });

  describe('edge cases', () => {
    it('should handle empty input', () => {
      const result = convertLLMToIX({});
      expect(result.ui.components).toHaveLength(0);
    });

    it('should handle null props', () => {
      const tree = { type: 'Box' };
      const result = convertLLMToIX(tree);
      expect(result.ui.components[0].props).toEqual({});
    });

    it('should handle deeply nested structures', () => {
      const deep = {
        type: 'Box',
        children: [
          {
            type: 'Box',
            children: [
              {
                type: 'Box',
                children: [{ type: 'Text' }],
              },
            ],
          },
        ],
      };

      const result = convertLLMToIX(deep);
      expect(result.ui.components[0].children?.[0].children?.[0].children).toHaveLength(1);
    });
  });
});
