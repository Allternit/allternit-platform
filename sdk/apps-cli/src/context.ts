export interface Context {
  cwd: string;
  env: Record<string, string | undefined>;
  log: (line: string) => void;
  error: (line: string) => void;
  fetch: typeof fetch;
}

export const nodeContext = (): Context => ({
  cwd: process.cwd(),
  env: process.env,
  log: (l) => console.log(l),
  error: (l) => console.error(l),
  fetch: globalThis.fetch,
});

/** Thrown for user-facing failures; the bin prints the message and exits 1. */
export class CliError extends Error {}
