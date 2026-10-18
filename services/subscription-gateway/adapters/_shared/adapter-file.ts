// An adapter's own data file (manifest.yaml, selectors/v1.yaml, …).
// Under tsx and vitest it sits next to the module (import.meta.url). In the
// bundle Allternit Desktop ships (surfaces/allternit-desktop/scripts/
// prepare-subs-gateway.cjs), an adapter imported by another adapter lands in
// a shared chunk, so the files are found from the bundle root instead
// (SUBS_GATEWAY_BUNDLE_ROOT, set by Desktop's gateway manager).
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export function adapterFile(importMetaUrl: string, adapterId: string, rel: string): URL {
  const root = process.env.SUBS_GATEWAY_BUNDLE_ROOT;
  return root ? pathToFileURL(join(root, "adapters", adapterId, rel)) : new URL(rel, importMetaUrl);
}
