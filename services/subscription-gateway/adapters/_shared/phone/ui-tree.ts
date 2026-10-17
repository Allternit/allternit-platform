// Parser for `phone.read_ui` text: one element per line, indented by depth, e.g.
//   iPhone:  "StaticText, value: 'Allternit iphone tree works', {{16, 229}, {361, 24}}"
//            "Button, label: 'Send', id: 'MessageInputPrimaryButtonContainerPart.PrimaryButton', {{333, 457}, {48, 48}}"
//   Android: "EditText, {{106, 1411}, {593, 84}}, clickable long-clickable editable"
//            "View, label: 'Send Message', {{632, 1432}, {42, 42}}"
// (shapes recorded from an iPhone 15 / iOS 26 and a moto g play / Android 14, 2026-10-17).

export interface UiNode {
  depth: number;
  cls: string;
  label?: string;
  value?: string;
  id?: string;
  frame?: { x: number; y: number; w: number; h: number };
  flags: string[];
}

const QUOTED = (key: string) => new RegExp(`${key}: '((?:[^'\\\\]|\\\\.)*)'`);

export function parseUiTree(text: string): UiNode[] {
  const out: UiNode[] = [];
  for (const raw of text.split("\n")) {
    if (!raw.trim()) continue;
    const depth = raw.length - raw.trimStart().length;
    const line = raw.trim();
    const cls = line.split(",")[0].trim();
    const label = QUOTED("label").exec(line)?.[1]?.replace(/\\'/g, "'");
    const value = QUOTED("value").exec(line)?.[1]?.replace(/\\'/g, "'");
    const id = QUOTED("id").exec(line)?.[1];
    const fm = /\{\{(-?\d+), (-?\d+)\}, \{(\d+), (\d+)\}\}/.exec(line);
    const frame = fm ? { x: +fm[1], y: +fm[2], w: +fm[3], h: +fm[4] } : undefined;
    const tail = fm ? line.slice(fm.index + fm[0].length).replace(/^,\s*/, "") : "";
    const flags = tail ? tail.split(/\s+/).filter(Boolean) : [];
    out.push({ depth, cls, label, value, id, frame, flags });
  }
  return out;
}

/** The text an element shows: value first (iOS static text), then label. */
export function nodeText(n: UiNode): string | undefined {
  return n.value ?? n.label;
}

export function findByLabel(nodes: UiNode[], re: RegExp): UiNode | undefined {
  return nodes.find((n) => (n.label !== undefined && re.test(n.label)) || (n.id !== undefined && re.test(n.id)));
}
