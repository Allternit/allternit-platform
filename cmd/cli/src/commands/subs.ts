import { Command } from 'commander';
import { SubsClient } from '../subs/client.js';

type GlobalOptions = { json?: boolean };

interface AccountRow {
  account_id: string;
  provider: string;
  label: string;
  plan: string | null;
  session_health: string;
  enabled: boolean;
}

// GET /v1/catalog entry (P4 phase 2) — subscription model published to the picker.
interface CatalogEntry {
  id: string;
  name: string;
  provider: string;
  tier: string;
  description: string;
  supports_effort: boolean;
  health: 'ready' | 'degraded';
  fabric: {
    adapter_id: string;
    account_id: string;
    capability: string;
    options: { model_class: string };
    pool_key: string;
  };
}

// GET /v1/capabilities entry, P4 shape (only the fields status uses).
interface CapabilityViewEntry {
  capability: string;
  adapter_id: string;
  provider: string;
  entitlements: {
    account_id: string;
    pool_key: string;
    pool_state: string;
    available: boolean;
    reason_unavailable?: string;
  }[];
}

// GET /v1/stats/adapters row (only the fields status uses).
interface AdapterStatsRow {
  adapter_id: string;
  adapter_version: string;
  attempts: number;
  success_rate: number;
}

function output(command: Command, value: unknown): void {
  const json = command.optsWithGlobals<GlobalOptions>().json;
  process.stdout.write(`${JSON.stringify(value, null, json ? 2 : 2)}\n`);
}

function printTable(rows: string[][]): void {
  if (rows.length === 0) return;
  const widths = rows[0].map((_, col) => Math.max(...rows.map((row) => row[col].length)));
  for (const row of rows) {
    process.stdout.write(`${row.map((cell, col) => cell.padEnd(widths[col])).join('  ').trimEnd()}\n`);
  }
}

async function run(command: Command, request: () => Promise<unknown>): Promise<void> {
  try {
    output(command, await request());
  } catch (error) {
    process.stderr.write(`allternit: ${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  }
}

export function createSubsCommand(): Command {
  const subs = new Command('subs').description('Subscription fabric accounts (subscription-gateway)');

  subs.addCommand(
    new Command('list')
      .description('List connected subscription accounts')
      .action(function (this: Command) {
        return run(this, () => new SubsClient().requestOk<AccountRow[]>('GET', '/v1/accounts'));
      }),
  );

  subs.addCommand(
    new Command('models')
      .description('List subscription model entries published to the model picker')
      .action(async function (this: Command) {
        const json = this.optsWithGlobals<GlobalOptions>().json;
        try {
          const entries = await new SubsClient().requestOk<CatalogEntry[]>('GET', '/v1/catalog');
          if (json) {
            output(this, entries);
          } else {
            printTable([['ID', 'NAME', 'HEALTH'], ...entries.map((e) => [e.id, e.name, e.health])]);
          }
        } catch (error) {
          process.stderr.write(`allternit: ${error instanceof Error ? error.message : String(error)}\n`);
          process.exitCode = 1;
        }
      }),
  );

  subs.addCommand(
    new Command('status')
      .description('Show per-account session health, pool state, and adapter stats')
      .action(async function (this: Command) {
        const client = new SubsClient();
        await run(this, async () => {
          const accounts = await client.requestOk<AccountRow[]>('GET', '/v1/accounts');
          const statuses = await Promise.all(
            accounts.map((a) =>
              client.requestOk('GET', `/v1/accounts/${a.account_id}/status`).catch(() => null),
            ),
          );
          // Older gateways predate these routes — degrade to empty rather than fail status.
          const capabilities = await client
            .requestOk<CapabilityViewEntry[]>('GET', '/v1/capabilities')
            .catch(() => [] as CapabilityViewEntry[]);
          const stats = await client
            .requestOk<AdapterStatsRow[]>('GET', '/v1/stats/adapters')
            .catch(() => [] as AdapterStatsRow[]);
          return accounts.map((a, i) => {
            const adapterIds = new Set(
              capabilities.filter((c) => c.provider === a.provider).map((c) => c.adapter_id),
            );
            return {
              ...a,
              status: statuses[i],
              pools: capabilities.flatMap((c) =>
                c.entitlements
                  .filter((en) => en.account_id === a.account_id)
                  .map((en) => ({ capability: c.capability, ...en })),
              ),
              adapter_stats: stats
                .filter((s) => adapterIds.has(s.adapter_id))
                .map((s) => ({
                  adapter_id: s.adapter_id,
                  adapter_version: s.adapter_version,
                  attempts: s.attempts,
                  success_rate: s.success_rate,
                })),
            };
          });
        });
      }),
  );

  subs.addCommand(
    new Command('connect')
      .description('Connect a provider account — the Sessions window opens for an interactive login')
      .argument('<provider>', 'provider id (see `allternit caps list`)')
      .option('--label <label>', 'account label')
      .action(function (this: Command, provider: string, options: { label?: string }) {
        return run(this, async () => {
          const client = new SubsClient();
          // Create-if-absent, then connect: one motion from the README flow.
          // Re-running after an interactive login re-drives the probe.
          const accounts = await client.requestOk<AccountRow[]>('GET', '/v1/accounts');
          let account = accounts.find((a) => a.provider === provider && a.enabled);
          if (!account) {
            account = await client.requestOk<AccountRow>('POST', '/v1/accounts', {
              provider,
              label: options.label ?? provider,
            });
            process.stderr.write(
              'Account created. Log in in the Sessions window that the gateway brings forward; ' +
                'health flips to ready once the probe passes.\n',
            );
          } else {
            process.stderr.write(
              'Using the existing account. Log in in the Sessions window that the gateway brings forward; ' +
                'health flips to ready once the probe passes.\n',
            );
          }
          return client.requestOk('POST', `/v1/accounts/${account.account_id}/connect`, {});
        });
      }),
  );

  subs.addCommand(
    new Command('disconnect')
      .description('Disable an account (does not wipe the profile)')
      .argument('<id>', 'account id')
      .action(function (this: Command, id: string) {
        return run(this, () => new SubsClient().requestOk('POST', `/v1/accounts/${id}/disconnect`));
      }),
  );

  return subs;
}

export const subsCommand = createSubsCommand();
