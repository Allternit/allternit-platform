/**
 * Parse `phone.read_ui` text (the helper's accessibility tree, one element per
 * line, indented by depth) into flat nodes the channel readers can match on:
 *
 *   Button, label: 'Back', id: 'com.x:id/b', {{14, 94}, {70, 70}}, clickable
 *
 * Labels and values are single-quoted with backslash escapes. Lines that don't
 * look like an element (window headers) are kept as type-only nodes.
 */

export interface UiNode {
  depth: number;
  type: string;
  label: string | null;
  value: string | null;
  id: string | null;
  rect: { x: number; y: number; w: number; h: number } | null;
  clickable: boolean;
  editable: boolean;
  selected: boolean;
}

const QUOTED = "'((?:[^'\\\\]|\\\\.)*)'";
const LINE = new RegExp(
  `^(\\s*)([A-Za-z_$][\\w$.]*)` +
    `(?:, label: ${QUOTED})?` +
    `(?:, value: ${QUOTED})?` +
    `(?:, id: ${QUOTED})?` +
    `(?:, \\{\\{(-?\\d+), (-?\\d+)\\}, \\{(\\d+), (\\d+)\\}\\})?` +
    `(.*)$`,
);

function unescape(s: string | undefined): string | null {
  if (s === undefined) return null;
  return s.replace(/\\n/g, '\n').replace(/\\(.)/g, '$1');
}

export function parseUiTree(text: string): UiNode[] {
  const out: UiNode[] = [];
  for (const line of text.split('\n')) {
    if (!line.trim()) continue;
    const m = LINE.exec(line);
    if (!m) continue;
    const flags = m[10] ?? '';
    out.push({
      depth: Math.floor(m[1].length / 2),
      type: m[2],
      label: unescape(m[3]),
      value: unescape(m[4]),
      id: unescape(m[5]),
      rect: m[6] !== undefined ? { x: Number(m[6]), y: Number(m[7]), w: Number(m[8]), h: Number(m[9]) } : null,
      clickable: /\bclickable\b/.test(flags),
      editable: /\beditable\b/.test(flags),
      selected: /\bselected\b/.test(flags),
    });
  }
  return out;
}

/** Nodes whose rect sits inside `outer` (descendants on screen, in tree order). */
export function nodesInside(nodes: UiNode[], outer: UiNode): UiNode[] {
  const o = outer.rect;
  if (!o) return [];
  return nodes.filter((n) => n !== outer && n.rect
    && n.rect.x >= o.x && n.rect.y >= o.y
    && n.rect.x + n.rect.w <= o.x + o.w && n.rect.y + n.rect.h <= o.y + o.h);
}

export const centerOf = (n: UiNode): { x: number; y: number } | null =>
  n.rect ? { x: n.rect.x + Math.floor(n.rect.w / 2), y: n.rect.y + Math.floor(n.rect.h / 2) } : null;
