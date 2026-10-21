/**
 * IX Capsule Runtime Tests
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  createIXCapsule,
  IXCapsuleRegistry,
  globalCapsuleRegistry,
} from '../runtime/capsule-runtime';
import { createStateStore } from '../state/store';
import type { UIRoot } from '../types';

describe('IX Capsule Runtime', () => {
  beforeEach(() => {
    globalCapsuleRegistry.clear();
  });

  const mockUI: UIRoot = {
    version: '1.0.0',
    components: [
      {
        id: 'btn1',
        type: 'Button',
        props: { children: 'Click me' },
      },
    ],
    state: { variables: [] },
    actions: [
      {
        id: 'submit',
        type: 'emitEvent',
        handler: { type: 'emitEvent', name: 'submit', payload: null },
      },
    ],
  };

  describe('createIXCapsule', () => {
    it('should create capsule with config', () => {
      const capsule = createIXCapsule({
        id: 'test-capsule',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      expect(capsule.id).toBe('test-capsule');
      expect(capsule.isActive()).toBe(true);
    });

    it('should get UI definition', () => {
      const capsule = createIXCapsule({
        id: 'test-capsule',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      const ui = capsule.getUI();
      expect(ui.version).toBe('1.0.0');
      expect(ui.components).toHaveLength(1);
    });

    it('should update UI', () => {
      const capsule = createIXCapsule({
        id: 'test-capsule',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      const newUI: UIRoot = {
        version: '1.0.0',
        components: [],
        state: { variables: [] },
        actions: [],
      };

      capsule.updateUI(newUI);
      expect(capsule.getUI().components).toHaveLength(0);
    });

    it('should manage state', () => {
      const capsule = createIXCapsule({
        id: 'test-capsule',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      capsule.setState('count', 42);
      expect(capsule.getState('count')).toBe(42);
    });

    it('should apply patches', () => {
      const capsule = createIXCapsule({
        id: 'test-capsule',
        ui: mockUI,
        stateStore: createStateStore({ initial: { count: 0 } }),
      });

      capsule.applyPatch([{ op: 'replace', path: '/count', value: 10 }]);
      expect(capsule.getState('count')).toBe(10);
    });

    it('should dispatch actions', async () => {
      const onEvent = vi.fn();
      const capsule = createIXCapsule({
        id: 'test-capsule',
        ui: mockUI,
        stateStore: createStateStore(),
        onEvent,
      });

      const result = await capsule.dispatch('submit', { data: 'test' });
      expect(result).toBe(true);
      expect(onEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'action',
          action: 'submit',
          params: { data: 'test' },
        })
      );
    });

    it('should throw for unknown actions', async () => {
      const capsule = createIXCapsule({
        id: 'test-capsule',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      await expect(capsule.dispatch('unknown')).rejects.toThrow('Action not found');
    });

    it('should subscribe to events', () => {
      const capsule = createIXCapsule({
        id: 'test-capsule',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      const callback = vi.fn();
      const unsubscribe = capsule.subscribe(callback);

      capsule.setState('test', 'value');
      expect(callback).toHaveBeenCalled();

      unsubscribe();
      callback.mockClear();
      capsule.setState('test2', 'value2');
      expect(callback).not.toHaveBeenCalled();
    });

    it('should emit lifecycle events', () => {
      const onEvent = vi.fn();
      const capsule = createIXCapsule({
        id: 'test-capsule',
        ui: mockUI,
        stateStore: createStateStore(),
        onEvent,
      });

      // Mount event on creation
      expect(onEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'lifecycle',
          lifecycle: 'mount',
        })
      );

      // Update event
      onEvent.mockClear();
      capsule.updateUI(mockUI);
      expect(onEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'lifecycle',
          lifecycle: 'update',
        })
      );

      // Unmount event on destroy
      onEvent.mockClear();
      capsule.destroy();
      expect(onEvent).toHaveBeenCalledWith(
        expect.objectContaining({
          type: 'lifecycle',
          lifecycle: 'unmount',
        })
      );
    });

    it('should mark as inactive after destroy', () => {
      const capsule = createIXCapsule({
        id: 'test-capsule',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      expect(capsule.isActive()).toBe(true);
      capsule.destroy();
      expect(capsule.isActive()).toBe(false);
    });

    it('should call onDestroy callback', () => {
      const onDestroy = vi.fn();
      const capsule = createIXCapsule({
        id: 'test-capsule',
        ui: mockUI,
        stateStore: createStateStore(),
        onDestroy,
      });

      capsule.destroy();
      expect(onDestroy).toHaveBeenCalled();
    });

    it('should not emit events after destroy', () => {
      const onEvent = vi.fn();
      const capsule = createIXCapsule({
        id: 'test-capsule',
        ui: mockUI,
        stateStore: createStateStore(),
        onEvent,
      });

      capsule.destroy();
      onEvent.mockClear();
      
      // Should not emit
      capsule.setState('test', 'value');
      expect(onEvent).not.toHaveBeenCalled();
    });
  });

  describe('IXCapsuleRegistry', () => {
    it('should create and register capsules', () => {
      const registry = new IXCapsuleRegistry();
      const capsule = registry.create({
        id: 'test',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      expect(registry.get('test')).toBe(capsule);
    });

    it('should check if capsule exists', () => {
      const registry = new IXCapsuleRegistry();
      registry.create({
        id: 'test',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      expect(registry.has('test')).toBe(true);
      expect(registry.has('unknown')).toBe(false);
    });

    it('should list all capsules', () => {
      const registry = new IXCapsuleRegistry();
      registry.create({
        id: 'test1',
        ui: mockUI,
        stateStore: createStateStore(),
      });
      registry.create({
        id: 'test2',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      expect(registry.list()).toHaveLength(2);
    });

    it('should list active capsules only', () => {
      const registry = new IXCapsuleRegistry();
      const capsule1 = registry.create({
        id: 'test1',
        ui: mockUI,
        stateStore: createStateStore(),
      });
      registry.create({
        id: 'test2',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      capsule1.destroy();
      expect(registry.getActive()).toHaveLength(1);
    });

    it('should remove and destroy capsules', () => {
      const registry = new IXCapsuleRegistry();
      const capsule = registry.create({
        id: 'test',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      expect(registry.remove('test')).toBe(true);
      expect(capsule.isActive()).toBe(false);
      expect(registry.has('test')).toBe(false);
    });

    it('should broadcast patches to all capsules', () => {
      const registry = new IXCapsuleRegistry();
      const capsule1 = registry.create({
        id: 'test1',
        ui: mockUI,
        stateStore: createStateStore(),
      });
      const capsule2 = registry.create({
        id: 'test2',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      registry.broadcastPatch([{ op: 'add', path: '/shared', value: 'data' }]);

      expect(capsule1.getState('shared')).toBe('data');
      expect(capsule2.getState('shared')).toBe('data');
    });

    it('should clear all capsules', () => {
      const registry = new IXCapsuleRegistry();
      const capsule = registry.create({
        id: 'test',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      registry.clear();
      expect(capsule.isActive()).toBe(false);
      expect(registry.list()).toHaveLength(0);
    });

    it('should set default policy gates', async () => {
      const registry = new IXCapsuleRegistry();
      const gate = {
        name: 'test-gate',
        check: vi.fn().mockReturnValue({ allowed: true }),
      };

      registry.setDefaultPolicyGates([gate]);
      const capsule = registry.create({
        id: 'test',
        ui: mockUI,
        stateStore: createStateStore(),
      });

      await capsule.dispatch('submit');
      expect(gate.check).toHaveBeenCalled();
    });
  });

  describe('globalCapsuleRegistry', () => {
    it('should be singleton', () => {
      expect(globalCapsuleRegistry).toBeDefined();
    });
  });
});
