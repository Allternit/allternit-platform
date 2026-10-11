/** Older Electron/Undici calls optional socket QoS outside the fetch promise.
 * macOS may reject that hint during socket teardown. Preserve real errors and
 * the connection; QoS failure must not crash an otherwise valid request.
 * Upstream: https://github.com/nodejs/undici/issues/5544
 */
export function installSocketQosGuard(socketPrototype: object): void {
  // Newer Node implements this optional method before older @types/node does.
  const prototype = socketPrototype as { setTypeOfService?: (...args: number[]) => unknown };
  const original = prototype.setTypeOfService;
  if (!original) return;
  prototype.setTypeOfService = function (this: unknown, ...args: number[]) {
    try {
      return original.apply(this, args);
    } catch (error) {
      const failure = error as { code?: string; syscall?: string };
      if (failure?.code === 'EINVAL' && failure.syscall === 'setTypeOfService') return this;
      throw error;
    }
  };
}
