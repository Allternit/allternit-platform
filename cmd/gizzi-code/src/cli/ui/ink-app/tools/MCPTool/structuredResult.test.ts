import { describe, expect, test } from 'bun:test';
import { alignLines, structuredView } from './structuredResult';

describe('structuredResult', () => {
  test('object becomes aligned key-value lines', () => {
    const v = structuredView({ id: 7, status: 'open' })!;
    expect(alignLines(v)).toEqual(['id      7', 'status  open']);
  });
  test('array of objects is a table capped at 10 rows plus "+N more"', () => {
    const rows = Array.from({ length: 13 }, (_, i) => ({ n: i, name: `r${i}` }));
    const lines = alignLines(structuredView(rows)!);
    expect(lines).toHaveLength(1 + 10 + 1);
    expect(lines[0]).toBe('n  name');
    expect(lines.at(-1)).toBe('+3 more');
  });
  test('single array key unwraps to a table; scalars and empties are not structured', () => {
    expect(structuredView({ items: [{ a: 1 }] })!.kind).toBe('table');
    expect(structuredView([1, 2])).toBeNull();
    expect(structuredView({})).toBeNull();
  });
});
