import React from 'react';
import { BLACK_CIRCLE, TOOL_GLYPHS, toolGlyphKind } from '../constants/figures';
import { useBlink } from '../hooks/useBlink';
import { Box, Text } from '../ink';
type Props = {
  isError: boolean;
  isUnresolved: boolean;
  shouldAnimate: boolean;
  toolName?: string;
};
export function ToolUseLoader({
    isError,
    isUnresolved,
    shouldAnimate,
    toolName
}: Props) {
  const [ref, isBlinking] = useBlink(shouldAnimate);
  const color = isUnresolved ? undefined : isError ? "error" : "success";
  const glyph = isUnresolved && !isError ? TOOL_GLYPHS[toolGlyphKind(toolName)] : BLACK_CIRCLE;
  const t1 = !shouldAnimate || isBlinking || isError || !isUnresolved ? (!isUnresolved && !isError ? "\u2713" : glyph) : " ";
  const t2 = <Text color={color} dimColor={isUnresolved}>{t1}</Text>;

  const t3 = <Box ref={ref} minWidth={2}>{t2}</Box>;

  return t3;
}
