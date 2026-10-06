import { RuntimeService } from '../../../runtime/runtime-service';

export async function runtimeStatusCommand(args: string[]): Promise<void> {
  const targetId = args[0];

  if (targetId) {
    const rt = await RuntimeService.get(targetId);
    if (!rt) {
      process.stderr.write(`Runtime not found: ${targetId}\n`);
      process.exit(1);
    }

    const ago = Math.round((Date.now() - (rt.lastHeartbeatAt ?? rt.registeredAt)) / 1000);
    process.stdout.write(`\nRuntime: ${rt.name} (${rt.id})\n`);
    process.stdout.write(`  Host:       ${rt.host}\n`);
    process.stdout.write(`  Status:     ${rt.status}\n`);
    process.stdout.write(`  Heartbeat:  ${ago}s ago\n`);
    process.stdout.write(`  Registered: ${new Date(rt.registeredAt).toLocaleString()}\n`);
    process.stdout.write(`  Agent CLIs: ${rt.agentClis.length}\n`);
    for (const cli of rt.agentClis) {
      process.stdout.write(`    • ${cli.name.padEnd(16)} ${cli.version}\n`);
    }
    process.stdout.write('\n');
    return;
  }

  const runtimes = await RuntimeService.list();
  const online = runtimes.filter((r) => r.status === 'online').length;
  const busy = runtimes.filter((r) => r.status === 'busy').length;
  const offline = runtimes.filter((r) => r.status === 'offline').length;

  process.stdout.write('\nRuntime registry status:\n');
  process.stdout.write(`  Total:   ${runtimes.length}\n`);
  process.stdout.write(`  Online:  ${online}\n`);
  process.stdout.write(`  Busy:    ${busy}\n`);
  process.stdout.write(`  Offline: ${offline}\n\n`);

  if (runtimes.length > 0) {
    process.stdout.write('Run `gizzi agents harness runtimes` to see details.\n');
  }
}
