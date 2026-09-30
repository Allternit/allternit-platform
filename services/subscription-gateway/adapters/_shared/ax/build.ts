// Hand-built AX tree helpers for offline fixtures (structure only; nothing recorded from a live app).
import type { AxNode } from "./types.js";
type Props = Partial<Omit<AxNode, "children" | "path" | "role">>;
export type Spec = { role: string; props?: Props; kids?: Spec[] };
export const n = (role: string, props: Props = {}, kids: Spec[] = []): Spec => ({ role, props, kids });
/** Materialize a Spec into an AxNode tree with `path` assigned (child indices from the root). */
export function build(s: Spec, path: number[] = []): AxNode {
  const node: AxNode = { role: s.role, ...s.props, path };
  if (s.kids?.length) node.children = s.kids.map((k, i) => build(k, [...path, i]));
  return node;
}
