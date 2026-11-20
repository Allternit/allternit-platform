/**
 * `gizzi runtime catalog [--all] [--json]` (Harness & Bots 2.0, plan D7).
 *
 * Prints the runtime registry (`runtimes[]` of cli-catalog.json): every
 * runtime a bot can run on, grouped by category. The table hides "More"
 * runtimes unless `--all`. `--json` prints every raw entry (each carries its
 * `more` flag) plus `installed` from the same CLI lookup discovery uses.
 */
import {
  listRuntimes,
  type RuntimeCategory,
  type RuntimeEntry,
} from '../../../runtime/providers/discovery/runtime-registry';
import { findSubprocessSpec, resolveCliPath } from '../../../runtime/providers/discovery/subprocess';

export const CATEGORY_ORDER: RuntimeCategory[] = ['native', 'coding', 'personal', 'general', 'experimental'];
export const CATEGORY_LABEL: Record<RuntimeCategory, string> = {
  native: 'Native',
  coding: 'Coding',
  personal: 'Personal',
  general: 'General',
  experimental: 'Experimental',
};

/**
 * Whether the runtime's CLI is on this machine: true for the built-in
 * Allternit runtime, null when the catalog names no CLI for it.
 */
export async function runtimeInstalled(r: RuntimeEntry): Promise<boolean | null> {
  if (r.adapter.kind === 'native') return true;
  if (!r.tool) return null;
  const spec = findSubprocessSpec(r.tool);
  if (!spec) return null;
  try {
    return (await resolveCliPath(spec)) !== null;
  } catch {
    return null;
  }
}

/** "any", or "locked: anthropic" for a runtime that runs one vendor's models. */
export function brainsLabel(r: RuntimeEntry): string {
  return r.brains === 'any' ? 'any' : `locked: ${r.brains.join(', ')}`;
}

export async function runtimeCatalogCommand(opts: { all?: boolean; json?: boolean } = {}): Promise<void> {
  const every = listRuntimes({ includeMore: true });
  const installed = await Promise.all(every.map(runtimeInstalled));

  if (opts.json) {
    const out = every.map((r, i) => (installed[i] === null ? r : { ...r, installed: installed[i] }));
    process.stdout.write(JSON.stringify(out, null, 2) + '\n');
    return;
  }

  const rows = every
    .map((r, i) => ({ r, installed: installed[i] }))
    .filter(({ r }) => opts.all || !r.more);
  const hidden = every.length - rows.length;

  const cells = (r: RuntimeEntry, inst: boolean | null): string[] => [
    r.id,
    r.name,
    r.adapter.kind,
    brainsLabel(r),
    r.access.map((a) => `[${a}]`).join(' '),
    inst === true ? (r.adapter.kind === 'native' ? 'built in' : 'installed') : inst === false ? 'not installed' : '—',
    r.status?.label ?? '',
  ];
  const header = ['ID', 'NAME', 'ADAPTER', 'BRAINS', 'ACCESS', 'LOCAL', 'STATUS'];
  const table = rows.map(({ r, installed: inst }) => cells(r, inst));
  const widths = header.map((h, c) => Math.max(h.length, ...table.map((row) => row[c].length)));
  const line = (cols: string[]) => cols.map((col, c) => col.padEnd(widths[c])).join('  ').trimEnd();

  const bold = '\x1b[1m';
  const dim = '\x1b[90m';
  const reset = '\x1b[0m';
  process.stdout.write(`\n  ${bold}${line(header)}${reset}\n`);
  for (const cat of CATEGORY_ORDER) {
    const inCat = rows.filter(({ r }) => r.category === cat);
    if (inCat.length === 0) continue;
    process.stdout.write(`\n${bold}${CATEGORY_LABEL[cat]}${reset} ${dim}(${inCat.length})${reset}\n`);
    for (const { r, installed: inst } of inCat) process.stdout.write(`  ${line(cells(r, inst))}\n`);
  }
  process.stdout.write('\n');
  if (hidden > 0) {
    process.stdout.write(`${dim}${hidden} more runtime${hidden === 1 ? '' : 's'} hidden. Show them with --all.${reset}\n`);
  }
  process.stdout.write(`${dim}Pick one for a bot: gizzi agents bot edit <name> --runtime <id>${reset}\n`);
}
