import React from 'react';
import { useTerminalSize } from '../hooks/useTerminalSize';
import { Text } from '../ink';
import { BLOCK_GLYPHS, BLOCK_PUNCT, BLOCK_ROWS } from './blockGlyphs';

const MIN_COLUMNS = 40;
const GLYPH_COLS = 5;
const GAP = 1;
const SPACE = 3;

/** Build the 3 half-block rows for a title, or null if a plain fallback is needed. */
export function renderBlockRows(title: string): string[] | null {
  const grid: boolean[][] = Array.from({ length: BLOCK_ROWS }, () => []);
  const put = (cells: ReadonlyArray<readonly [number, number]>, x0: number, w: number) => {
    for (const row of grid) for (let i = 0; i < w; i++) row[x0 + i] = false;
    for (const [cx, cy] of cells) grid[cy]![x0 + cx] = true;
  };
  let x = 0;
  let first = true;
  for (const raw of title) {
    const ch = raw.toUpperCase();
    if (ch === ' ') {
      x += SPACE + (first ? 0 : GAP);
      continue;
    }
    const glyph = BLOCK_GLYPHS[ch];
    const punct = BLOCK_PUNCT[ch];
    if (!glyph && !punct) return null;
    if (!first) x += GAP;
    const w = glyph ? GLYPH_COLS : punct!.w;
    put(glyph ?? punct!.cells, x, w);
    x += w;
    first = false;
  }
  const lit = (r: number, c: number) => (r < BLOCK_ROWS && grid[r]![c]) === true;
  const rows: string[] = [];
  for (let r = 0; r < BLOCK_ROWS; r += 2) {
    let line = '';
    for (let c = 0; c < x; c++) {
      const top = lit(r, c);
      const bottom = lit(r + 1, c);
      line += top && bottom ? '█' : top ? '▀' : bottom ? '▄' : ' ';
    }
    rows.push(line.replace(/\s+$/, ''));
  }
  return rows;
}

/** A title in the A:// block type, drawn with half-blocks (3 terminal rows). */
export function BlockTitle({ title }: { title: string }): React.ReactNode {
  const { columns } = useTerminalSize();
  const plain = columns < MIN_COLUMNS || process.env.NO_COLOR !== undefined || !process.stdout.isTTY;
  const rows = plain ? null : renderBlockRows(title);
  if (!rows || Math.max(...rows.map(r => r.length)) > columns - 4) {
    return <Text bold>{title}</Text>;
  }
  return <Text color="text">{rows.join('\n')}</Text>;
}
