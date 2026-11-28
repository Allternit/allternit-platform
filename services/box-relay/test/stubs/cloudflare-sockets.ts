// Stand-in for the workerd `cloudflare:sockets` module under vitest. The relay
// logic takes connect() as a dependency, so no test reaches this.
export function connect(): never {
  throw new Error("cloudflare:sockets is not available in tests");
}
