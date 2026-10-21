/**
 * Policy Gates Tests
 */

import { describe, it, expect, vi } from 'vitest';
import {
  createRateLimitGate,
  createActionAllowlistGate,
  createComponentRestrictionsGate,
  createStateValidationGate,
  createPropSanitizationGate,
  createResourceLimitGate,
  createAuditLogGate,
  composePolicyGates,
  createDefaultPolicyGates,
} from '../runtime/policy-gates';
import type { PolicyContext } from '../runtime/policy-gates';

describe('Policy Gates', () => {
  const mockContext: PolicyContext = {
    capsuleId: 'test-capsule',
    timestamp: Date.now(),
  };

  describe('createRateLimitGate', () => {
    it('should allow actions within limit', async () => {
      const gate = createRateLimitGate({ maxActionsPerMinute: 5 });

      for (let i = 0; i < 5; i++) {
        const result = await gate.check('action', mockContext);
        expect(result.allowed).toBe(true);
      }
    });

    it('should deny actions exceeding limit', async () => {
      const gate = createRateLimitGate({ maxActionsPerMinute: 2 });

      // Use up limit
      await gate.check('action', mockContext);
      await gate.check('action', mockContext);

      // Third should be denied
      const result = await gate.check('action', mockContext);
      expect(result.allowed).toBe(false);
      expect(result.reason).toContain('Rate limit exceeded');
    });

    it('should allow non-action checks', async () => {
      const gate = createRateLimitGate({ maxActionsPerMinute: 1 });

      const result = await gate.check('render', mockContext);
      expect(result.allowed).toBe(true);
    });
  });

  describe('createActionAllowlistGate', () => {
    it('should allow whitelisted actions', async () => {
      const gate = createActionAllowlistGate(['submit', 'cancel']);

      const result = await gate.check('action', {
        ...mockContext,
        actionId: 'submit',
      });
      expect(result.allowed).toBe(true);
    });

    it('should deny non-whitelisted actions', async () => {
      const gate = createActionAllowlistGate(['submit']);

      const result = await gate.check('action', {
        ...mockContext,
        actionId: 'delete',
      });
      expect(result.allowed).toBe(false);
      expect(result.reason).toContain('not in allowlist');
    });

    it('should allow non-action checks', async () => {
      const gate = createActionAllowlistGate(['submit']);

      const result = await gate.check('state-change', mockContext);
      expect(result.allowed).toBe(true);
    });
  });

  describe('createComponentRestrictionsGate', () => {
    it('should allow whitelisted components', async () => {
      const gate = createComponentRestrictionsGate({
        allowedComponents: ['Button', 'Input'],
      });

      const result = await gate.check('render', {
        ...mockContext,
        component: 'Button',
      });
      expect(result.allowed).toBe(true);
    });

    it('should deny non-whitelisted components', async () => {
      const gate = createComponentRestrictionsGate({
        allowedComponents: ['Button'],
      });

      const result = await gate.check('render', {
        ...mockContext,
        component: 'Script',
      });
      expect(result.allowed).toBe(false);
      expect(result.reason).toContain('not in allowlist');
    });

    it('should deny blocked components', async () => {
      const gate = createComponentRestrictionsGate({
        blockedComponents: ['Script', 'Iframe'],
      });

      const result = await gate.check('render', {
        ...mockContext,
        component: 'Script',
      });
      expect(result.allowed).toBe(false);
      expect(result.reason).toContain('blocked');
    });

    it('should allow non-render checks', async () => {
      const gate = createComponentRestrictionsGate({
        allowedComponents: ['Button'],
      });

      const result = await gate.check('action', mockContext);
      expect(result.allowed).toBe(true);
    });
  });

  describe('createStateValidationGate', () => {
    it('should validate required fields', async () => {
      const gate = createStateValidationGate({
        name: { type: 'string', required: true },
      });

      const result = await gate.check('state-change', {
        ...mockContext,
        state: {},
      });
      expect(result.allowed).toBe(false);
      expect(result.reason).toContain('required');
    });

    it('should validate types', async () => {
      const gate = createStateValidationGate({
        count: { type: 'number' },
      });

      const result = await gate.check('state-change', {
        ...mockContext,
        state: { count: 'not a number' },
      });
      expect(result.allowed).toBe(false);
      expect(result.reason).toContain('must be number');
    });

    it('should run custom validation', async () => {
      const gate = createStateValidationGate({
        email: {
          type: 'string',
          validate: (value) =>
            typeof value === 'string' && value.includes('@') ? true : 'Invalid email',
        },
      });

      const valid = await gate.check('state-change', {
        ...mockContext,
        state: { email: 'test@example.com' },
      });
      expect(valid.allowed).toBe(true);

      const invalid = await gate.check('state-change', {
        ...mockContext,
        state: { email: 'invalid' },
      });
      expect(invalid.allowed).toBe(false);
      expect(invalid.reason).toBe('Invalid email');
    });

    it('should allow non-state-change checks', async () => {
      const gate = createStateValidationGate({
        name: { type: 'string', required: true },
      });

      const result = await gate.check('action', mockContext);
      expect(result.allowed).toBe(true);
    });
  });

  describe('createPropSanitizationGate', () => {
    it('should strip HTML by default', async () => {
      const gate = createPropSanitizationGate({});

      const result = await gate.check('render', {
        ...mockContext,
        props: { text: '<p>Hello</p>' },
      });
      expect(result.context?.props?.text).toBe('Hello');
    });

    it('should allow specific HTML tags when enabled', async () => {
      const gate = createPropSanitizationGate({
        allowHTML: true,
        allowedTags: ['b', 'i'],
      });

      const result = await gate.check('render', {
        ...mockContext,
        props: { text: '<b>Bold</b> <script>alert(1)</script>' },
      });
      expect(result.context?.props?.text).toBe('<b>Bold</b> ');
    });

    it('should remove event handlers', async () => {
      const gate = createPropSanitizationGate({});

      const result = await gate.check('render', {
        ...mockContext,
        props: { onClick: 'alert(1)' },
      });
      expect(result.context?.props?.onClick).toBeUndefined();
    });

    it('should remove javascript: URLs', async () => {
      const gate = createPropSanitizationGate({
        allowHTML: true,
      });

      const result = await gate.check('render', {
        ...mockContext,
        props: { href: 'javascript:alert(1)' },
      });
      expect(result.context?.props?.href).toBe('alert(1)');
    });
  });

  describe('createResourceLimitGate', () => {
    it('should enforce max state size', async () => {
      const gate = createResourceLimitGate({ maxStateSize: 100 });

      const result = await gate.check('state-change', {
        ...mockContext,
        state: { data: 'x'.repeat(200) },
      });
      expect(result.allowed).toBe(false);
      expect(result.reason).toContain('State size');
    });

    it('should enforce max action count', async () => {
      const gate = createResourceLimitGate({ maxActions: 2 });

      await gate.check('action', mockContext);
      await gate.check('action', mockContext);
      
      const result = await gate.check('action', mockContext);
      expect(result.allowed).toBe(false);
      expect(result.reason).toContain('Maximum action count');
    });

    it('should allow within limits', async () => {
      const gate = createResourceLimitGate({
        maxStateSize: 1000,
        maxActions: 100,
      });

      const result = await gate.check('state-change', {
        ...mockContext,
        state: { data: 'small' },
      });
      expect(result.allowed).toBe(true);
    });
  });

  describe('createAuditLogGate', () => {
    it('should log events', async () => {
      const logFn = vi.fn();
      const gate = createAuditLogGate({ logFn });

      await gate.check('action', {
        ...mockContext,
        actionId: 'submit',
      });

      expect(logFn).toHaveBeenCalledWith(
        expect.objectContaining({
          capsuleId: 'test-capsule',
          actionId: 'submit',
          type: 'action',
        })
      );
    });

    it('should respect logAllowed option', async () => {
      const logFn = vi.fn();
      const gate = createAuditLogGate({ logFn, logAllowed: false });

      await gate.check('render', mockContext);
      expect(logFn).not.toHaveBeenCalled();
    });
  });

  describe('composePolicyGates', () => {
    it('should pass all gates', async () => {
      const gate1 = { name: 'gate1', check: vi.fn().mockReturnValue({ allowed: true }) };
      const gate2 = { name: 'gate2', check: vi.fn().mockReturnValue({ allowed: true }) };

      const composed = composePolicyGates(gate1, gate2);
      const result = await composed.check('action', mockContext);

      expect(result.allowed).toBe(true);
      expect(gate1.check).toHaveBeenCalled();
      expect(gate2.check).toHaveBeenCalled();
    });

    it('should fail on first denial', async () => {
      const gate1 = { name: 'gate1', check: vi.fn().mockReturnValue({ allowed: true }) };
      const gate2 = {
        name: 'gate2',
        check: vi.fn().mockReturnValue({ allowed: false, reason: 'Denied' }),
      };
      const gate3 = { name: 'gate3', check: vi.fn().mockReturnValue({ allowed: true }) };

      const composed = composePolicyGates(gate1, gate2, gate3);
      const result = await composed.check('action', mockContext);

      expect(result.allowed).toBe(false);
      expect(result.reason).toBe('Denied');
      expect(gate3.check).not.toHaveBeenCalled();
    });

    it('should propagate context modifications', async () => {
      const gate1 = {
        name: 'gate1',
        check: vi.fn().mockReturnValue({
          allowed: true,
          context: { ...mockContext, modified: true } as PolicyContext,
        }),
      };
      const gate2 = { name: 'gate2', check: vi.fn().mockReturnValue({ allowed: true }) };

      const composed = composePolicyGates(gate1, gate2);
      await composed.check('action', mockContext);

      expect(gate2.check).toHaveBeenCalledWith(
        'action',
        expect.objectContaining({ modified: true })
      );
    });
  });

  describe('createDefaultPolicyGates', () => {
    it('should create default gates', () => {
      const gates = createDefaultPolicyGates();
      expect(gates).toHaveLength(3);
    });

    it('should include rate limiting', async () => {
      const gates = createDefaultPolicyGates();
      const rateGate = gates[0];
      
      // Should allow within default limits
      const result = await rateGate.check('action', mockContext);
      expect(result.allowed).toBe(true);
    });

    it('should include sanitization', async () => {
      const gates = createDefaultPolicyGates();
      const sanitizationGate = gates[1];

      const result = await sanitizationGate.check('render', {
        ...mockContext,
        props: { text: '<script>alert(1)</script>' },
      });
      expect(result.context?.props?.text).toBe('alert(1)');
    });

    it('should include resource limits', async () => {
      const gates = createDefaultPolicyGates();
      const resourceGate = gates[2];

      const largeState = { data: 'x'.repeat(2 * 1024 * 1024) }; // 2MB
      const result = await resourceGate.check('state-change', {
        ...mockContext,
        state: largeState,
      });
      expect(result.allowed).toBe(false);
    });
  });
});
