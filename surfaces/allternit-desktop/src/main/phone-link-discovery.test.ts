import { describe, expect, it } from 'vitest';
import { findPhoneHost, netmaskToPrefix, subnetHosts } from './phone-link-discovery.js';

describe('subnet math', () => {
  it('converts netmasks to prefix lengths', () => {
    expect(netmaskToPrefix('255.255.255.0')).toBe(24);
    expect(netmaskToPrefix('255.255.254.0')).toBe(23);
    expect(netmaskToPrefix('255.255.252.0')).toBe(22);
    expect(netmaskToPrefix('255.255.0.0')).toBe(16);
    expect(netmaskToPrefix('255.0.255.0')).toBeNull(); // non-contiguous
    expect(netmaskToPrefix('0.0.0.0')).toBe(0);
  });

  it('lists a /24 without network and broadcast addresses', () => {
    expect(subnetHosts('192.168.1.1', '255.255.255.0')).toHaveLength(254);
    expect(subnetHosts('192.168.1.1', '255.255.255.0')).toContain('192.168.1.77');
    expect(subnetHosts('192.168.1.1', '255.255.255.0')).not.toContain('192.168.1.0');
    expect(subnetHosts('192.168.1.1', '255.255.255.0')).not.toContain('192.168.1.255');
  });

  it('lists a /23 and caps a /22 at 1022 hosts', () => {
    const slash23 = subnetHosts('10.0.0.7', '255.255.254.0');
    expect(slash23).toHaveLength(510);
    expect(slash23[0]).toBe('10.0.0.1');
    expect(slash23.at(-1)).toBe('10.0.1.254');

    const slash22 = subnetHosts('10.0.0.7', '255.255.252.0');
    expect(slash22).toHaveLength(1022);
    expect(slash22[0]).toBe('10.0.0.1');
    expect(slash22.at(-1)).toBe('10.0.3.254');
  });

  it('skips subnets larger than a /22 and junk input', () => {
    expect(subnetHosts('10.0.0.7', '255.255.0.0')).toEqual([]); // /16
    expect(subnetHosts('10.0.0.7', '0.0.0.0')).toEqual([]); // /0
    expect(subnetHosts('not-an-ip', '255.255.255.0')).toEqual([]);
  });
});

describe('findPhoneHost', () => {
  const iface = { address: '10.0.0.5', netmask: '255.255.255.0' };

  it('returns the last host first when it still answers', async () => {
    const probed: string[] = [];
    const found = await findPhoneHost('dev-1', '10.0.0.90:8200', {
      lanInterfaces: () => [iface],
      fetchWhoami: async (host) => {
        probed.push(host);
        return { device_id: 'dev-1' };
      },
    });
    expect(found).toBe('10.0.0.90:8200');
    expect(probed).toEqual(['10.0.0.90:8200']);
  });

  it('scans the subnet when the last host is stale, skipping its own address', async () => {
    const probed: string[] = [];
    const found = await findPhoneHost('dev-1', '10.0.0.90:8200', {
      lanInterfaces: () => [iface],
      fetchWhoami: async (host) => {
        probed.push(host);
        return host === '10.0.0.11:8200' ? { device_id: 'dev-1' } : null;
      },
    });
    expect(found).toBe('10.0.0.11:8200');
    expect(probed[0]).toBe('10.0.0.90:8200');
    expect(probed.filter((h) => h === '10.0.0.90:8200')).toHaveLength(1); // probed once, not re-scanned
    expect(probed).not.toContain('10.0.0.5:8200'); // this computer
    expect(probed).toContain('10.0.0.11:8200');
  });

  it('never trusts a host that answers with another device id', async () => {
    const found = await findPhoneHost('dev-1', undefined, {
      lanInterfaces: () => [{ address: '10.0.1.5', netmask: '255.255.255.0' }],
      fetchWhoami: async () => ({ device_id: 'impostor' }),
    });
    expect(found).toBeNull();
  });

  it('returns null when nobody answers', async () => {
    const found = await findPhoneHost('dev-1', undefined, {
      lanInterfaces: () => [],
      fetchWhoami: async () => null,
    });
    expect(found).toBeNull();
  });

  it('works without a last host and appends the default port', async () => {
    const probed: string[] = [];
    const found = await findPhoneHost('dev-1', undefined, {
      lanInterfaces: () => [{ address: '10.0.1.5', netmask: '255.255.255.0' }],
      fetchWhoami: async (host) => {
        probed.push(host);
        return host === '10.0.1.42:8200' ? { device_id: 'dev-1' } : null;
      },
    });
    expect(found).toBe('10.0.1.42:8200');
    expect(probed.every((h) => h.endsWith(':8200'))).toBe(true);
  });
});

describe('scanLan', () => {
  it('records every device id that answers, first host wins', async () => {
    const { scanLan } = await import('./phone-link-discovery.js');
    const found = await scanLan({
      lanInterfaces: () => [{ address: '10.0.0.5', netmask: '255.255.255.0' }],
      fetchWhoami: async (h: string) => ({ '10.0.0.11:8200': { device_id: 'a' }, '10.0.0.252:8200': { device_id: 'b' } } as Record<string, { device_id: string }>)[h] ?? null,
    });
    expect(Object.fromEntries(found)).toEqual({ a: '10.0.0.11:8200', b: '10.0.0.252:8200' });
  });
});
