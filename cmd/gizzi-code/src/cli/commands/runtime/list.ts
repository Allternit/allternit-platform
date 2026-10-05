import { RuntimeService } from '../../../runtime/runtime-service';

export async function runtimeListCommand(): Promise<void> {
  const runtimes = await RuntimeService.list();

  if (runtimes.length === 0) {
    process.stdout.write('No runtimes registered.\n');
    process.stdout.write('Run `gizzi agents harness register` to discover local agent CLIs.\n');
    return;
  }

  process.stdout.write(`\nRegistered runtimes (${runtimes.length}):\n\n`);

  for (const rt of runtimes) {
    const statusIcon = rt.status === 'online' ? '●' : rt.status === 'busy' ? '◎' : '○';
    const statusColor = rt.status === 'online' ? '\x1b[32m' : rt.status === 'busy' ? '\x1b[33m' : '\x1b[90m';
    const reset = '\x1b[0m';

    process.stdout.write(
      `  ${statusColor}${statusIcon}${reset} ${rt.name.padEnd(20)} ${rt.host.padEnd(20)} ` +
      `${rt.agentClis.length} CLI${rt.agentClis.length !== 1 ? 's' : ''}\n`
    );

    for (const cli of rt.agentClis) {
      process.stdout.write(`      ${'\x1b[36m'}✓${reset} ${cli.name.padEnd(16)} ${cli.version}\n`);
    }
    process.stdout.write('\n');
  }
}
