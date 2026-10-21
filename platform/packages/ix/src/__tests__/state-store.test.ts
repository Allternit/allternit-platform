/**
 * State Store Tests
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { createStateStore } from '../state/store';

// Mock localStorage with actual storage
const storage: Record<string, string> = {};
const localStorageMock = {
  getItem: vi.fn((key: string) => storage[key] ?? null),
  setItem: vi.fn((key: string, value: string) => { storage[key] = value; }),
  clear: vi.fn(() => { Object.keys(storage).forEach(k => delete storage[k]); }),
  removeItem: vi.fn((key: string) => { delete storage[key]; }),
};

Object.defineProperty(globalThis, 'localStorage', {
  value: localStorageMock,
  writable: true,
});

describe('StateStore', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorageMock.clear();
  });

  describe('basic operations', () => {
    it('should create store with initial values', () => {
      const store = createStateStore({
        initial: { name: 'test', count: 5 },
      });

      expect(store.get('name')).toBe('test');
      expect(store.get('count')).toBe(5);
    });

    it('should set and get values', () => {
      const store = createStateStore();
      
      store.set('foo', 'bar');
      expect(store.get('foo')).toBe('bar');

      store.set('nested.value', 42);
      expect(store.get('nested.value')).toBe(42);
    });

    it('should get full snapshot', () => {
      const store = createStateStore({
        initial: { a: 1, b: 2 },
      });

      const snapshot = store.snapshot();
      expect(snapshot).toEqual({ a: 1, b: 2 });
    });

    it('should restore from snapshot', () => {
      const store = createStateStore();
      
      store.restore({ x: 10, y: 20 });
      expect(store.get('x')).toBe(10);
      expect(store.get('y')).toBe(20);
    });
  });

  describe('nested paths', () => {
    it('should handle nested paths', () => {
      const store = createStateStore();
      
      store.set('user.profile.name', 'John');
      expect(store.get('user.profile.name')).toBe('John');
      
      store.set('user.profile.age', 30);
      expect(store.get('user')).toEqual({
        profile: { name: 'John', age: 30 },
      });
    });

    it('should create nested objects automatically', () => {
      const store = createStateStore();
      
      store.set('a.b.c.d', 'deep');
      expect(store.get('a.b.c.d')).toBe('deep');
    });
  });

  describe('subscriptions', () => {
    it('should notify subscribers on change', () => {
      const store = createStateStore();
      const callback = vi.fn();
      
      store.subscribe('test', callback);
      store.set('test', 'value1');
      
      expect(callback).toHaveBeenCalledWith('value1');
    });

    it('should allow unsubscribing', () => {
      const store = createStateStore();
      const callback = vi.fn();
      
      const unsubscribe = store.subscribe('test', callback);
      unsubscribe();
      
      store.set('test', 'value');
      expect(callback).not.toHaveBeenCalled();
    });

    it('should notify parent path subscribers', () => {
      const store = createStateStore();
      const parentCallback = vi.fn();
      
      store.subscribe('user', parentCallback);
      store.set('user.name', 'John');
      
      expect(parentCallback).toHaveBeenCalled();
    });
  });

  describe('computed values', () => {
    it('should compute derived values', () => {
      const store = createStateStore({
        initial: { a: 5, b: 10 },
      });
      
      store.compute('sum', ['a', 'b'], () => {
        return (store.get('a') as number) + (store.get('b') as number);
      });
      
      expect(store.get('sum')).toBe(15);
    });

    it('should recompute when dependencies change', () => {
      const store = createStateStore({
        initial: { a: 5 },
      });
      
      const computeFn = vi.fn(() => store.get('a'));
      store.compute('doubled', ['a'], () => (store.get('a') as number) * 2);
      
      expect(store.get('doubled')).toBe(10);
      
      store.set('a', 20);
      expect(store.get('doubled')).toBe(40);
    });
  });

  describe('bindings', () => {
    it('should register component bindings', () => {
      const store = createStateStore();
      
      store.bind('comp1', 'value', 'form.input');
      store.bind('comp1', 'disabled', 'form.loading');
      
      const bindings = store.getBindings('comp1');
      expect(bindings).toHaveLength(2);
      expect(bindings).toContainEqual({ prop: 'value', statePath: 'form.input' });
      expect(bindings).toContainEqual({ prop: 'disabled', statePath: 'form.loading' });
    });
  });

  describe('batch updates', () => {
    it('should batch multiple updates', () => {
      const store = createStateStore();
      const callback = vi.fn();
      
      store.subscribe('a', callback);
      store.subscribe('b', callback);
      
      store.batch({ a: 1, b: 2 });
      
      expect(store.get('a')).toBe(1);
      expect(store.get('b')).toBe(2);
      expect(callback).toHaveBeenCalledTimes(2);
    });
  });

  describe('reset', () => {
    it('should reset to initial values', () => {
      const store = createStateStore({
        initial: { x: 1, y: 2 },
      });
      
      store.set('x', 100);
      store.set('z', 300);
      
      store.reset();
      
      expect(store.get('x')).toBe(1);
      expect(store.get('y')).toBe(2);
      expect(store.get('z')).toBeUndefined();
    });
  });

  describe('persistence', () => {
    it('should persist to localStorage', () => {
      const store = createStateStore({
        scopeId: 'test-scope',
        persist: true,
      });
      
      store.set('key', 'value');
      
      // Simulate new store instance
      const store2 = createStateStore({
        scopeId: 'test-scope',
        persist: true,
      });
      
      expect(store2.get('key')).toBe('value');
    });
  });
});
