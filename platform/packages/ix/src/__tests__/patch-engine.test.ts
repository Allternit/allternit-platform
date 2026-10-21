/**
 * JSON Patch Engine Tests (RFC 6902)
 */

import { describe, it, expect } from 'vitest';
import {
  applyPatch,
  applyOperation,
  generatePatch,
  invertPatch,
  validatePatch,
  validateOperation,
  getValueAtPath,
  setValueAtPath,
  decodeJsonPointer,
  encodeJsonPointer,
} from '../state/patch';
import type { JSONPatch, JSONPatchOperation } from '../state/patch';

describe('JSON Patch Engine', () => {
  describe('JSON Pointer helpers', () => {
    it('should decode JSON Pointer', () => {
      expect(decodeJsonPointer('')).toEqual([]);
      expect(decodeJsonPointer('/')).toEqual([]);
      expect(decodeJsonPointer('/foo')).toEqual(['foo']);
      expect(decodeJsonPointer('/foo/bar')).toEqual(['foo', 'bar']);
      expect(decodeJsonPointer('/foo~1bar')).toEqual(['foo/bar']);
      expect(decodeJsonPointer('/foo~0bar')).toEqual(['foo~bar']);
    });

    it('should encode JSON Pointer', () => {
      expect(encodeJsonPointer([])).toBe('');
      expect(encodeJsonPointer(['foo'])).toBe('/foo');
      expect(encodeJsonPointer(['foo', 'bar'])).toBe('/foo/bar');
      expect(encodeJsonPointer(['foo/bar'])).toBe('/foo~1bar');
      expect(encodeJsonPointer(['foo~bar'])).toBe('/foo~0bar');
    });

    it('should throw on invalid pointer', () => {
      expect(() => decodeJsonPointer('invalid')).toThrow();
    });
  });

  describe('getValueAtPath', () => {
    const obj = {
      foo: 'bar',
      nested: { deep: { value: 42 } },
      array: [1, 2, 3],
    };

    it('should get values at paths', () => {
      expect(getValueAtPath(obj, '/foo')).toBe('bar');
      expect(getValueAtPath(obj, '/nested/deep/value')).toBe(42);
      expect(getValueAtPath(obj, '/array/0')).toBe(1);
      expect(getValueAtPath(obj, '/array/-')).toBeUndefined();
    });

    it('should return undefined for non-existent paths', () => {
      expect(getValueAtPath(obj, '/nonexistent')).toBeUndefined();
      expect(getValueAtPath(obj, '/nested/nonexistent/deep')).toBeUndefined();
    });
  });

  describe('setValueAtPath', () => {
    it('should add values at paths', () => {
      const obj = { foo: 'bar' };
      const result = setValueAtPath(obj, '/baz', 'qux', 'add');
      expect(result).toEqual({ foo: 'bar', baz: 'qux' });
    });

    it('should replace values at paths', () => {
      const obj = { foo: 'bar' };
      const result = setValueAtPath(obj, '/foo', 'baz', 'replace');
      expect(result).toEqual({ foo: 'baz' });
    });

    it('should remove values at paths', () => {
      const obj = { foo: 'bar', baz: 'qux' };
      const result = setValueAtPath(obj, '/baz', undefined, 'remove');
      expect(result).toEqual({ foo: 'bar' });
    });

    it('should handle array indices', () => {
      const obj = { arr: [1, 2, 3] };
      const result = setValueAtPath(obj, '/arr/1', 'replaced', 'replace');
      expect(result).toEqual({ arr: [1, 'replaced', 3] });
    });

    it('should append to array with -', () => {
      const obj = { arr: [1, 2] };
      const result = setValueAtPath(obj, '/arr/-', 3, 'add');
      expect(result).toEqual({ arr: [1, 2, 3] });
    });
  });

  describe('applyOperation', () => {
    describe('add', () => {
      it('should add property', () => {
        const state = { foo: 'bar' };
        const result = applyOperation(state, { op: 'add', path: '/baz', value: 'qux' });
        expect(result.success).toBe(true);
        expect(result.newState).toEqual({ foo: 'bar', baz: 'qux' });
      });

      it('should fail without value', () => {
        const state = { foo: 'bar' };
        // @ts-expect-error - testing invalid operation
        const result = applyOperation(state, { op: 'add', path: '/baz' });
        expect(result.success).toBe(false);
      });
    });

    describe('remove', () => {
      it('should remove property', () => {
        const state = { foo: 'bar', baz: 'qux' };
        const result = applyOperation(state, { op: 'remove', path: '/baz' });
        expect(result.success).toBe(true);
        expect(result.newState).toEqual({ foo: 'bar' });
      });

      it('should fail if path does not exist', () => {
        const state = { foo: 'bar' };
        const result = applyOperation(state, { op: 'remove', path: '/nonexistent' });
        expect(result.success).toBe(false);
      });
    });

    describe('replace', () => {
      it('should replace property', () => {
        const state = { foo: 'bar' };
        const result = applyOperation(state, { op: 'replace', path: '/foo', value: 'baz' });
        expect(result.success).toBe(true);
        expect(result.newState).toEqual({ foo: 'baz' });
      });

      it('should fail if path does not exist', () => {
        const state = { foo: 'bar' };
        const result = applyOperation(state, { op: 'replace', path: '/nonexistent', value: 'baz' });
        expect(result.success).toBe(false);
      });
    });

    describe('move', () => {
      it('should move value from one path to another', () => {
        const state = { foo: 'bar', baz: { qux: 'quux' } };
        const result = applyOperation(state, { op: 'move', path: '/baz/copied', from: '/foo' });
        expect(result.success).toBe(true);
        expect(result.newState).toEqual({ baz: { qux: 'quux', copied: 'bar' } });
      });

      it('should fail if from path does not exist', () => {
        const state = { foo: 'bar' };
        const result = applyOperation(state, { op: 'move', path: '/baz', from: '/nonexistent' });
        expect(result.success).toBe(false);
      });
    });

    describe('copy', () => {
      it('should copy value', () => {
        const state = { foo: { bar: 'baz' } };
        const result = applyOperation(state, { op: 'copy', path: '/qux', from: '/foo' });
        expect(result.success).toBe(true);
        expect(result.newState).toEqual({
          foo: { bar: 'baz' },
          qux: { bar: 'baz' },
        });
      });

      it('should deep copy objects', () => {
        const state = { obj: { nested: 'value' } };
        const result = applyOperation(state, { op: 'copy', path: '/copy', from: '/obj' });
        // Modify original to ensure deep copy
        (result.newState as typeof state).obj.nested = 'modified';
        expect((result.newState as typeof state).copy.nested).toBe('value');
      });
    });

    describe('test', () => {
      it('should pass when values match', () => {
        const state = { foo: 'bar' };
        const result = applyOperation(state, { op: 'test', path: '/foo', value: 'bar' });
        expect(result.success).toBe(true);
      });

      it('should fail when values do not match', () => {
        const state = { foo: 'bar' };
        const result = applyOperation(state, { op: 'test', path: '/foo', value: 'baz' });
        expect(result.success).toBe(false);
      });
    });
  });

  describe('applyPatch', () => {
    it('should apply multiple operations', () => {
      const state = { foo: 'bar' };
      const patch: JSONPatch = [
        { op: 'add', path: '/baz', value: 'qux' },
        { op: 'replace', path: '/foo', value: 'updated' },
      ];
      
      const result = applyPatch(state, patch);
      expect(result.success).toBe(true);
      expect(result.newState).toEqual({ foo: 'updated', baz: 'qux' });
    });

    it('should fail atomically on first error', () => {
      const state = { foo: 'bar' };
      const patch: JSONPatch = [
        { op: 'add', path: '/baz', value: 'qux' },
        { op: 'remove', path: '/nonexistent' },
        { op: 'add', path: '/quux', value: 'corge' },
      ];
      
      const result = applyPatch(state, patch);
      expect(result.success).toBe(false);
      expect(result.failedAt).toBe(1);
      // State should remain unchanged
      expect(state).toEqual({ foo: 'bar' });
    });
  });

  describe('generatePatch', () => {
    it('should generate add operation for new property', () => {
      const from = { foo: 'bar' };
      const to = { foo: 'bar', baz: 'qux' };
      
      const patch = generatePatch(from, to);
      expect(patch).toContainEqual({ op: 'add', path: '/baz', value: 'qux' });
    });

    it('should generate remove operation for deleted property', () => {
      const from = { foo: 'bar', baz: 'qux' };
      const to = { foo: 'bar' };
      
      const patch = generatePatch(from, to);
      expect(patch).toContainEqual({ op: 'remove', path: '/baz' });
    });

    it('should generate replace operation for changed value', () => {
      const from = { foo: 'bar' };
      const to = { foo: 'baz' };
      
      const patch = generatePatch(from, to);
      expect(patch).toContainEqual({ op: 'replace', path: '/foo', value: 'baz' });
    });

    it('should handle nested objects', () => {
      const from = { nested: { a: 1 } };
      const to = { nested: { a: 1, b: 2 } };
      
      const patch = generatePatch(from, to);
      expect(patch).toContainEqual({ op: 'add', path: '/nested/b', value: 2 });
    });

    it('should handle arrays', () => {
      const from = { arr: [1, 2] };
      const to = { arr: [1, 2, 3] };
      
      const patch = generatePatch(from, to);
      expect(patch).toContainEqual({ op: 'add', path: '/arr/2', value: 3 });
    });

    it('should handle array element removal', () => {
      const from = { arr: [1, 2, 3] };
      const to = { arr: [1, 2] };
      
      const patch = generatePatch(from, to);
      expect(patch).toContainEqual({ op: 'remove', path: '/arr/2' });
    });
  });

  describe('invertPatch', () => {
    it('should invert add to remove', () => {
      const patch: JSONPatch = [{ op: 'add', path: '/foo', value: 'bar' }];
      const state = {};
      
      const inverted = invertPatch(patch, state);
      expect(inverted).toEqual([{ op: 'remove', path: '/foo' }]);
    });

    it('should invert remove to add with original value', () => {
      const patch: JSONPatch = [{ op: 'remove', path: '/foo' }];
      const state = { foo: 'original' };
      
      const inverted = invertPatch(patch, state);
      expect(inverted).toEqual([{ op: 'add', path: '/foo', value: 'original' }]);
    });

    it('should invert replace with original value', () => {
      const patch: JSONPatch = [{ op: 'replace', path: '/foo', value: 'new' }];
      const state = { foo: 'original' };
      
      const inverted = invertPatch(patch, state);
      expect(inverted).toEqual([{ op: 'replace', path: '/foo', value: 'original' }]);
    });

    it('should invert move by swapping paths', () => {
      const patch: JSONPatch = [{ op: 'move', path: '/to', from: '/from' }];
      const state = { from: 'value' };
      
      const inverted = invertPatch(patch, state);
      expect(inverted).toEqual([{ op: 'move', path: '/from', from: '/to' }]);
    });

    it('should invert copy to remove', () => {
      const patch: JSONPatch = [{ op: 'copy', path: '/to', from: '/from' }];
      const state = { from: 'value', to: 'value' };
      
      const inverted = invertPatch(patch, state);
      expect(inverted).toEqual([{ op: 'remove', path: '/to' }]);
    });

    it('should keep test operations unchanged', () => {
      const patch: JSONPatch = [{ op: 'test', path: '/foo', value: 'bar' }];
      const state = {};
      
      const inverted = invertPatch(patch, state);
      expect(inverted).toEqual([{ op: 'test', path: '/foo', value: 'bar' }]);
    });

    it('should invert operations in reverse order', () => {
      const patch: JSONPatch = [
        { op: 'add', path: '/a', value: 1 },
        { op: 'add', path: '/b', value: 2 },
      ];
      const state = {};
      
      const inverted = invertPatch(patch, state);
      expect(inverted).toEqual([
        { op: 'remove', path: '/b' },
        { op: 'remove', path: '/a' },
      ]);
    });
  });

  describe('validation', () => {
    describe('validateOperation', () => {
      it('should validate valid operations', () => {
        const op: JSONPatchOperation = { op: 'add', path: '/foo', value: 'bar' };
        expect(validateOperation(op)).toEqual({ valid: true });
      });

      it('should reject invalid operation type', () => {
        const op = { op: 'invalid', path: '/foo' } as JSONPatchOperation;
        expect(validateOperation(op)).toEqual({
          valid: false,
          error: 'Invalid operation: invalid',
        });
      });

      it('should reject invalid path', () => {
        const op = { op: 'add', path: 'invalid', value: 'bar' } as JSONPatchOperation;
        expect(validateOperation(op)).toEqual({
          valid: false,
          error: 'Invalid path: invalid',
        });
      });

      it('should require value for add/replace/test', () => {
        const op = { op: 'add', path: '/foo' } as JSONPatchOperation;
        expect(validateOperation(op)).toEqual({
          valid: false,
          error: 'add operation requires value',
        });
      });

      it('should require from path for move/copy', () => {
        const op = { op: 'move', path: '/to' } as JSONPatchOperation;
        expect(validateOperation(op)).toEqual({
          valid: false,
          error: 'move operation requires from path',
        });
      });
    });

    describe('validatePatch', () => {
      it('should validate valid patch', () => {
        const patch: JSONPatch = [
          { op: 'add', path: '/foo', value: 'bar' },
          { op: 'replace', path: '/baz', value: 'qux' },
        ];
        expect(validatePatch(patch)).toEqual({ valid: true });
      });

      it('should return index of first invalid operation', () => {
        const patch: JSONPatch = [
          { op: 'add', path: '/foo', value: 'bar' },
          { op: 'invalid', path: '/baz' } as JSONPatchOperation,
        ];
        expect(validatePatch(patch)).toEqual({
          valid: false,
          error: 'Invalid operation: invalid',
          index: 1,
        });
      });
    });
  });
});
