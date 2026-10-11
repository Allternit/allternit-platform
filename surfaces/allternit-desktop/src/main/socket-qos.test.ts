import { describe, expect, it, vi } from 'vitest';
import { installSocketQosGuard } from './socket-qos.js';
describe('socket QoS compatibility', () => {
  it('keeps requests alive when optional QoS is rejected', () => {
    const prototype = { setTypeOfService: () => { throw Object.assign(new Error('QoS'), { code: 'EINVAL', syscall: 'setTypeOfService' }); } };
    installSocketQosGuard(prototype);
    expect(prototype.setTypeOfService()).toBe(prototype);
  });
  it('preserves successful calls and receiver', () => {
    const original = vi.fn(function (this: unknown, _value: number) { return this; });
    const prototype = { setTypeOfService: original };
    installSocketQosGuard(prototype);
    expect(prototype.setTypeOfService(0)).toBe(prototype);
    expect(original).toHaveBeenCalledWith(0);
  });
  it('does not hide connection or unrelated errors', () => {
    const error = Object.assign(new Error('connection'), { code: 'ECONNRESET', syscall: 'write' });
    const prototype = { setTypeOfService: () => { throw error; } };
    installSocketQosGuard(prototype);
    expect(() => prototype.setTypeOfService()).toThrow(error);
    installSocketQosGuard({});
  });
});
