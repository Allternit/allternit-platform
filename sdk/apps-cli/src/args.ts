export interface Parsed {
  positional: string[];
  flags: Record<string, string | boolean>;
}

/** `--key value`, `--key=value`, `--flag`; everything else is positional. No dependencies on purpose. */
export function parseArgs(argv: string[], booleans: string[] = []): Parsed {
  const positional: string[] = [];
  const flags: Record<string, string | boolean> = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) {
      positional.push(a);
      continue;
    }
    const eq = a.indexOf("=");
    if (eq > 0) {
      flags[a.slice(2, eq)] = a.slice(eq + 1);
      continue;
    }
    const key = a.slice(2);
    const next = argv[i + 1];
    if (booleans.includes(key) || next === undefined || next.startsWith("--")) flags[key] = true;
    else {
      flags[key] = next;
      i++;
    }
  }
  return { positional, flags };
}

export const str = (v: string | boolean | undefined): string | undefined => (typeof v === "string" ? v : undefined);
