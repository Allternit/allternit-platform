import { feature } from 'bun:bundle';
import figures from 'figures';
import React, { useEffect, useRef, useState } from 'react';
import { useTerminalSize } from '../hooks/useTerminalSize';
import { stringWidth } from '../ink/stringWidth';
import { Box, Text } from '../ink';
import { useAppState, useSetAppState } from '../state/AppState';
import type { AppState } from '../state/AppStateStore';
import { getGlobalConfig } from '../utils/config';
import { isFullscreenActive } from '../utils/fullscreen';
import type { Theme } from '../utils/theme';
import { getCompanion } from './companion';
import type { GizziPose } from './gizziSprite';
import { petBotRows, petFaceColors } from './botSprite';
import { refreshPetBots, useCurrentPetBot } from './petBots';
import { AVATAR_COLS, AVATAR_ROWS, useAvatarPng } from './avatarImage';
import { inlineImageBlock, inlineImageClear, inlineImageProtocol } from '../ink/inlineImage';
const TICK_MS = 500;
const BUBBLE_SHOW = 20; // ticks → ~10s at 500ms
const FADE_WINDOW = 6; // last ~3s the bubble dims so you know it's about to go
const PET_BURST_MS = 2500; // how long hearts float after /pet pat

// Idle sequence: mostly rest, an occasional glance up at the beacon, a rare
// blink and wink (the Gizzi mark's animation states).
const IDLE_SEQUENCE: GizziPose[] = ['idle', 'idle', 'idle', 'idle', 'glance', 'glance', 'idle', 'idle', 'blink', 'idle', 'idle', 'wink', 'idle', 'idle', 'idle'];
// Excited (speaking or petted): look around quickly.
const EXCITED_SEQUENCE: GizziPose[] = ['idle', 'glance', 'idle', 'wink'];

// Hearts float up-and-out over 5 ticks (~2.5s). Prepended above the sprite.
const H = figures.heart;
const PET_HEARTS = [`   ${H}    ${H}   `, `  ${H}  ${H}   ${H}  `, ` ${H}   ${H}  ${H}   `, `${H}  ${H}      ${H} `, '·    ·   ·  '];
function wrap(text: string, width: number): string[] {
  const words = text.split(' ');
  const lines: string[] = [];
  let cur = '';
  for (const w of words) {
    if (cur.length + w.length + 1 > width && cur) {
      lines.push(cur);
      cur = w;
    } else {
      cur = cur ? `${cur} ${w}` : w;
    }
  }
  if (cur) lines.push(cur);
  return lines;
}
function SpeechBubble(t0) {
  const {
    text,
    color,
    fading,
    tail
  } = t0;
  const lines = wrap(text, 30);
  const borderColor = fading ? "inactive" : color;
  const T0 = Box;
  const t1 = "column";
  const t2 = "round";
  const t3 = borderColor;
  const t4 = 1;
  const t5 = 34;
  const t7 = (l, i) => <Text key={i} italic={true} dimColor={!fading} color={fading ? "inactive" : undefined}>{l}</Text>;

  const t6 = lines.map(t7);

  const t7_2 = <T0 flexDirection={t1} borderStyle={t2} borderColor={t3} paddingX={t4} width={t5}>{t6}</T0>;

  const bubble = t7_2;
  if (tail === "right") {
    const t8 = <Text color={borderColor}>─</Text>;

    const t9 = <Box flexDirection="row" alignItems="center">{bubble}{t8}</Box>;

    return t9;
  }
  const t8 = <Box flexDirection="column" alignItems="flex-end" paddingRight={6}><Text color={borderColor}>╲ </Text><Text color={borderColor}>╲</Text></Box>;

  const t9 = <Box flexDirection="column" alignItems="flex-end" marginRight={1}>{bubble}{t8}</Box>;

  return t9;
}
/**
 * A bot's image avatar drawn as an inline image in the sprite's cells. Kitty
 * and Ghostty keep an image on screen until it's deleted, so it's taken down
 * when the pet stops showing it (narrow terminal, muted, another bot).
 */
function AvatarImage({ imageKey, rows }: { imageKey: string; rows: string[] }): React.ReactNode {
  useEffect(() => () => {
    const clear = inlineImageClear(imageKey);
    if (clear) process.stdout.write(clear);
  }, [imageKey]);
  return <>{rows.map((row, i) => <Text key={i}>{row}</Text>)}</>;
}
export const MIN_COLS_FOR_FULL_SPRITE = 100;
const SPRITE_BODY_WIDTH = 12;
const NAME_ROW_PAD = 2; // focused state wraps name in spaces: ` name `
const SPRITE_PADDING_X = 2;
const BUBBLE_WIDTH = 36; // SpeechBubble box (34) + tail column
const NARROW_QUIP_CAP = 24;
function spriteColWidth(nameWidth: number): number {
  return Math.max(SPRITE_BODY_WIDTH, nameWidth + NAME_ROW_PAD);
}

// Width the sprite area consumes. PromptInput subtracts this so text wraps
// correctly. In fullscreen the bubble floats over scrollback (no extra
// width); in non-fullscreen it sits inline and needs BUBBLE_WIDTH more.
// Narrow terminals: 0 — REPL.tsx stacks the one-liner on its own row
// (above input in fullscreen, below in scrollback), so no reservation.
export function companionReservedColumns(terminalColumns: number, speaking: boolean): number {
  if (!feature('PET')) return 0;
  const companion = getCompanion();
  if (!companion || getGlobalConfig().companionMuted) return 0;
  if (terminalColumns < MIN_COLS_FOR_FULL_SPRITE) return 0;
  const nameWidth = stringWidth(companion.name);
  const bubble = speaking && !isFullscreenActive() ? BUBBLE_WIDTH : 0;
  return spriteColWidth(nameWidth) + SPRITE_PADDING_X + bubble;
}
export function CompanionSprite(): React.ReactNode {
  const reaction = useAppState(s => s.companionReaction);
  const petAt = useAppState(s => s.companionPetAt);
  const focused = useAppState(s => s.footerSelection === 'companion');
  const setAppState = useSetAppState();
  // Re-render when the worn bot changes (HUD pick, Desktop pet, roster load).
  const wornBot = useCurrentPetBot();
  // Only fetched where the terminal can draw it; elsewhere the pet is Gizzi.
  const avatarUrl = feature('PET') && wornBot.avatar.kind === 'image' && inlineImageProtocol() ? wornBot.avatar.url : undefined;
  const avatarPng = useAvatarPng(avatarUrl);
  useEffect(() => {
    if (feature('PET')) void refreshPetBots();
  }, []);
  const {
    columns
  } = useTerminalSize();
  const [tick, setTick] = useState(0);
  const lastSpokeTick = useRef(0);
  // Sync-during-render (not useEffect) so the first post-pet render already
  // has petStartTick=tick and petAge=0 — otherwise frame 0 is skipped.
  const [{
    petStartTick,
    forPetAt
  }, setPetStart] = useState({
    petStartTick: 0,
    forPetAt: petAt
  });
  if (petAt !== forPetAt) {
    setPetStart({
      petStartTick: tick,
      forPetAt: petAt
    });
  }
  useEffect(() => {
    const timer = setInterval(setT => setT((t: number) => t + 1), TICK_MS, setTick);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => {
    if (!reaction) return;
    lastSpokeTick.current = tick;
    const timer = setTimeout(setA => setA((prev: AppState) => prev.companionReaction === undefined ? prev : {
      ...prev,
      companionReaction: undefined
    }), BUBBLE_SHOW * TICK_MS, setAppState);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- tick intentionally captured at reaction-change, not tracked
  }, [reaction, setAppState]);
  if (!feature('PET')) return null;
  const companion = getCompanion();
  if (!companion || getGlobalConfig().companionMuted) return null;
  const color = companion.bot.accent ?? 'gizzi';
  const colWidth = spriteColWidth(stringWidth(companion.name));
  const bubbleAge = reaction ? tick - lastSpokeTick.current : 0;
  const fading = reaction !== undefined && bubbleAge >= BUBBLE_SHOW - FADE_WINDOW;
  const petAge = petAt ? tick - petStartTick : Infinity;
  const petting = petAge * TICK_MS < PET_BURST_MS;

  // Narrow terminals: collapse to one-line face. When speaking, the quip
  // replaces the name beside the face (no room for a bubble).
  if (columns < MIN_COLS_FOR_FULL_SPRITE) {
    const quip = reaction && reaction.length > NARROW_QUIP_CAP ? reaction.slice(0, NARROW_QUIP_CAP - 1) + '…' : reaction;
    const label = quip ? `"${quip}"` : focused ? ` ${companion.name} ` : companion.name;
    const face = petFaceColors(companion.bot);
    return <Box paddingX={1} alignSelf="flex-end">
        <Text>
          {petting && <Text color="autoAccept">{figures.heart} </Text>}
          <Text color={face.body}>▐</Text>
          <Text color={face.eye} backgroundColor={face.face}>■ ■</Text>
          <Text color={face.body}>▌</Text>{' '}
          <Text italic dimColor={!focused && !reaction} bold={focused} inverse={focused && !reaction} color={reaction ? fading ? 'inactive' : color : focused ? color : undefined}>
            {label}
          </Text>
        </Text>
      </Box>;
  }
  const heartFrame = petting ? PET_HEARTS[petAge % PET_HEARTS.length] : null;
  const excited = reaction !== undefined || petting;
  const pose = excited ? EXCITED_SEQUENCE[tick % EXCITED_SEQUENCE.length]! : IDLE_SEQUENCE[tick % IDLE_SEQUENCE.length]!;
  // The beacon (Gizzi) or antenna (robot pets) pulses while excited.
  const body = petBotRows(companion.bot, pose, excited && tick % 2 === 1 ? 'gizziShimmer' : 'gizzi');
  // Image avatars in terminals that draw images; the Gizzi rows above otherwise.
  const imageKey = avatarUrl && avatarPng && companion.bot.avatar.kind === 'image' && companion.bot.avatar.url === avatarUrl ? `pet-avatar:${avatarUrl}` : undefined;
  const imageRows = imageKey && avatarPng ? inlineImageBlock(imageKey, avatarPng, AVATAR_COLS, AVATAR_ROWS) : null;

  // Name row doubles as hint row — unfocused shows dim name + ↓ discovery,
  // focused shows inverse name. The enter-to-open hint lives in
  // PromptInputFooter's right column so this row stays one line and the
  // sprite doesn't jump up when selected. flexShrink=0 stops the
  // inline-bubble row wrapper from squeezing the sprite to fit.
  const spriteColumn = <Box flexDirection="column" flexShrink={0} alignItems="center" width={colWidth}>
      {heartFrame && <Text color="autoAccept">{heartFrame}</Text>}
      {/* Fixed-width, left-aligned box: rows ending in transparent cells get
          their trailing spaces trimmed, and centering the shorter row would
          shift it sideways and bend the sprite. */}
      <Box flexDirection="column" width={SPRITE_BODY_WIDTH} flexShrink={0}>
        {imageKey && imageRows ? <AvatarImage imageKey={imageKey} rows={imageRows} /> : body.map((segments, i) => <Text key={i}>
            {segments.map(([text, fg, bg], j) => <Text key={j} color={fg || undefined} backgroundColor={bg}>{text}</Text>)}
          </Text>)}
      </Box>
      <Text italic bold={focused} dimColor={!focused} color={focused ? color : undefined} inverse={focused}>
        {focused ? ` ${companion.name} ` : companion.name}
      </Text>
    </Box>;
  if (!reaction) {
    return <Box paddingX={1}>{spriteColumn}</Box>;
  }

  // Fullscreen: bubble renders separately via CompanionFloatingBubble in
  // FullscreenLayout's bottomFloat slot (the bottom slot's overflowY:hidden
  // would clip a position:absolute overlay here). Sprite body only.
  // Non-fullscreen: bubble sits inline beside the sprite (input shrinks)
  // because floating into Static scrollback can't be cleared.
  if (isFullscreenActive()) {
    return <Box paddingX={1}>{spriteColumn}</Box>;
  }
  return <Box flexDirection="row" alignItems="flex-end" paddingX={1} flexShrink={0}>
      <SpeechBubble text={reaction} color={color} fading={fading} tail="right" />
      {spriteColumn}
    </Box>;
}

// Floating bubble overlay for fullscreen mode. Mounted in FullscreenLayout's
// bottomFloat slot (outside the overflowY:hidden clip) so it can extend into
// the ScrollBox region. CompanionSprite owns the clear-after-10s timer; this
// just reads companionReaction and renders the fade.
export function CompanionFloatingBubble() {
  const reaction = useAppState(_temp);
  const t0 = {
      tick: 0,
      forReaction: reaction
    };

  const [t1, setTick] = useState(t0);
  const {
    tick,
    forReaction
  } = t1;
  if (reaction !== forReaction) {
    setTick({
      tick: 0,
      forReaction: reaction
    });
  }
  const t2 = () => {
      if (!reaction) {
        return;
      }
      const timer = setInterval(_temp3, TICK_MS, setTick);
      return () => clearInterval(timer);
    };
  const t3 = [reaction];

  useEffect(t2, t3);
  if (!feature("PET") || !reaction) {
    return null;
  }
  const companion = getCompanion();
  if (!companion || getGlobalConfig().companionMuted) {
    return null;
  }
  const t4 = tick >= BUBBLE_SHOW - FADE_WINDOW;
  const t5 = <SpeechBubble text={reaction} color={companion.bot.accent ?? 'gizzi'} fading={t4} tail="down" />;

  return t5;
}
function _temp3(set) {
  return set(_temp2);
}
function _temp2(s_0) {
  return {
    ...s_0,
    tick: s_0.tick + 1
  };
}
function _temp(s) {
  return s.companionReaction;
}
