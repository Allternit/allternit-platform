import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { existsSync } from 'node:fs';
import { subprocessSpecs, type SubprocessSpec, resolveCliPath, PROVIDER_ENV_KEYS } from '@/runtime/providers/discovery/subprocess';

const execFileAsync = promisify(execFile);

export { PROVIDER_ENV_KEYS };

export interface DiscoveredCli {
  name: string;
  path: string;
  version: string;
  icon: string;
}

export interface DiscoveredRuntime {
  host: string;
  agentClis: DiscoveredCli[];
  discoveredAt: number;
}

async function getCliVersion(path: string): Promise<string> {
  for (const flag of ['--version', 'version', '-v']) {
    try {
      const { stdout, stderr } = await execFileAsync(path, [flag], { timeout: 3000 });
      const raw = (stdout + stderr).trim().split('\n')[0];
      if (raw) return raw.slice(0, 80);
    } catch {
      // try next flag
    }
  }
  return 'unknown';
}

async function runProbe(bin: string, spec: SubprocessSpec): Promise<boolean> {
  if (!spec.probe) return true;
  try {
    const { stdout, stderr } = await execFileAsync(bin, spec.probe.args, { timeout: 3000 });
    const out = (stdout + stderr).trim();
    const { expect } = spec.probe;
    return typeof expect === 'string' ? out.includes(expect) : expect.test(out);
  } catch {
    return false;
  }
}

export async function discoverLocalAgentClis(): Promise<DiscoveredCli[]> {
  const results: DiscoveredCli[] = [];

  await Promise.all(
    // Catalog CLIs plus the user's own (custom-clis.json); a listed CLI with
    // no driver adapter yet isn't registered as runnable.
    subprocessSpecs().filter((spec) => spec.adapter !== 'unsupported').map(async (spec) => {
      const path = await resolveCliPath(spec);
      if (!path) return;
      const alive = await runProbe(path, spec);
      if (!alive) return;
      const version = await getCliVersion(path);
      results.push({ name: spec.id, path, version, icon: spec.icon ?? spec.id });
    })
  );

  return results;
}

export async function discoverLocalRuntime(host = 'localhost'): Promise<DiscoveredRuntime> {
  const agentClis = await discoverLocalAgentClis();
  return {
    host,
    agentClis,
    discoveredAt: Date.now(),
  };
}

export function formatDiscoveryReport(runtime: DiscoveredRuntime): string {
  const lines: string[] = [
    `Runtime host: ${runtime.host}`,
    `Discovered at: ${new Date(runtime.discoveredAt).toLocaleString()}`,
    `Agent CLIs found: ${runtime.agentClis.length}`,
    '',
  ];
  if (runtime.agentClis.length === 0) {
    lines.push('  (none found)');
  } else {
    for (const cli of runtime.agentClis) {
      lines.push(`  ✓ ${cli.name.padEnd(18)} ${cli.version}  (${cli.path})`);
    }
  }
  return lines.join('\n');
}
