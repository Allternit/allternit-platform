import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useInterval } from 'usehooks-ts';
import {
  createIncognitoThread,
  ensureStandingThread,
  followThread,
  listSessionMessages,
  sendThreadTurn,
  threadApi,
  turnContextTokens,
  type PlatformThread,
} from '@/runtime/bots/platform-threads.js';
import { PlatformSignedOutError } from '@/runtime/bots/platform-api.js';
import { getSessionPaused, pausedLine, type SessionPaused } from '@/runtime/bots/session-pause.js';
import type { CommandResultDisplay } from '../commands';
import TextInput from '../components/TextInput';
import { SpinnerGlyph } from '../components/Spinner/SpinnerGlyph';
import { SessionRip } from '../components/messages/SessionRip';
import { useTerminalSize } from '../hooks/useTerminalSize';
import { Box, Text, useInput } from '../ink';
import { useSetAppState } from '../state/AppState';
import { errorMessage } from '../utils/errors';
import { askPetLocally, type ChatLine } from './localChat';
import { earlierWindowAtTop, hudLinesFromMessages, isChat, maxOffset, visibleLines, type HudLine } from './hudLines';
import { refreshPetBots, selectPetBot, usePetBots, type PetBot, GIZZI_BOT } from './petBots';

/**
 * The pet HUD: a small bordered panel for quick work with the bot the pet
 * wears. Tab cycles three views:
 *  - Thread: the bot's standing thread — the same thread Desktop's Threads
 *    panel shows, steered from here (spec BOT_THREAD_PARITY_SPEC.md).
 *  - Incognito: a quick ask that is never saved (an incognito thread when
 *    signed in; gizzi's own model offline).
 *  - Bots: pick which bot to wear, here and on the Desktop pet.
 */
export type PetHudTab = 'thread' | 'incognito' | 'bots';
const TABS: Array<{ id: PetHudTab; label: string }> = [
  { id: 'thread', label: 'Thread' },
  { id: 'incognito', label: 'Incognito' },
  { id: 'bots', label: 'Bots' },
];
const SHOWN_LINES = 4;
const MAX_CHARS = 480;
const LOCAL_TIMEOUT_MS = 90_000;

type Props = {
  onDone: (result?: string, options?: { display?: CommandResultDisplay }) => void;
};

function clip(text: string): string {
  const flat = text.trim();
  return flat.length > MAX_CHARS ? `${flat.slice(0, MAX_CHARS - 1)}…` : flat;
}

function Lines({ lines, offset = 0, bot, accent, width }: { lines: HudLine[]; offset?: number; bot: PetBot; accent: string; width: number }) {
  return <Box flexDirection="column">
      {visibleLines(lines, offset, SHOWN_LINES).map((line, i) => !isChat(line) ? <SessionRip key={i} compact startedAt={line.at} generation={line.generation} reason={line.reason} width={width} /> : <Box key={i} flexDirection="row">
          <Box width={Math.max(4, bot.name.length) + 2} flexShrink={0}>
            {line.role === 'user' ? <Text dimColor>you</Text> : <Text color={accent}>{bot.name}</Text>}
          </Box>
          <Box flexGrow={1} flexShrink={1}>
            <Text wrap="wrap">{clip(line.content)}</Text>
          </Box>
        </Box>)}
    </Box>;
}

export function PetHud({ onDone }: Props): React.ReactNode {
  const { bots, currentId, connection, expired, loading } = usePetBots();
  const online = connection === 'online';
  const bot = bots.find(b => b.id === currentId) ?? GIZZI_BOT;
  const accent = bot.accent ?? 'gizzi';
  const { columns } = useTerminalSize();

  const [tab, setTabState] = useState<PetHudTab>('thread');
  const tabRef = useRef<PetHudTab>('thread');
  const setTab = useCallback((next: PetHudTab) => {
    tabRef.current = next;
    setTabState(next);
  }, []);
  const [input, setInput] = useState('');
  const [cursor, setCursor] = useState(0);
  const [busy, setBusy] = useState(false);
  const [frame, setFrame] = useState(0);
  const [error, setError] = useState<string | null>(null);
  // Paused before a usage limit (P3.17), per view: the thread's window or the incognito ask's.
  const [threadPaused, setThreadPaused] = useState<SessionPaused | null>(null);
  const [incognitoPaused, setIncognitoPaused] = useState<SessionPaused | null>(null);
  const [thread, setThread] = useState<PlatformThread | null>(null);
  const [threadLines, setThreadLines] = useState<HudLine[]>([]);
  // Thread scrollback: lines scrolled up from the newest, and the earlier
  // windows (by session id) already loaded above their rips.
  const [scroll, setScroll] = useState(0);
  const [earlier, setEarlier] = useState<{ loaded: Set<string>; state: 'idle' | 'loading' | 'error' }>({ loaded: new Set(), state: 'idle' });
  const [incognitoLines, setIncognitoLines] = useState<ChatLine[]>([]);
  const [botCursor, setBotCursorState] = useState(0);
  // Keys can arrive in one burst (Up then Enter); Enter must see the moved
  // cursor, not the value from the last render.
  const botCursorRef = useRef(0);
  const setBotCursor = useCallback((next: number | ((c: number) => number)) => {
    botCursorRef.current = typeof next === 'function' ? next(botCursorRef.current) : next;
    setBotCursorState(botCursorRef.current);
  }, []);
  const incognitoThread = useRef<PlatformThread | null>(null);
  const inFlight = useRef<AbortController | null>(null);

  useInterval(() => setFrame(f => f + 1), busy ? 80 : null);

  const setAppState = useSetAppState();
  useEffect(() => {
    setAppState(prev => ({ ...prev, petHudOpen: true, footerSelection: null }));
    return () => setAppState(prev => ({ ...prev, petHudOpen: false }));
  }, [setAppState]);

  useEffect(() => {
    void refreshPetBots();
  }, []);

  // While paused, re-check every 30s: gizzi resumes on its own at the reset.
  useEffect(() => {
    if (!threadPaused && !incognitoPaused) return;
    const timer = setInterval(() => {
      const threadSid = thread?.currentSessionId;
      if (threadPaused && threadSid) void getSessionPaused(threadSid).then(setThreadPaused).catch(() => {});
      const incSid = incognitoThread.current?.currentSessionId;
      if (incognitoPaused && incSid) void getSessionPaused(incSid).then(setIncognitoPaused).catch(() => {});
    }, 30_000);
    return () => clearInterval(timer);
  }, [threadPaused, incognitoPaused, thread?.currentSessionId]);

  // Load the worn bot's standing thread (and its recent lines) when signed in.
  useEffect(() => {
    setThread(null);
    setThreadLines([]);
    setScroll(0);
    setEarlier({ loaded: new Set(), state: 'idle' });
    setThreadPaused(null);
    if (!online) return;
    let cancelled = false;
    void (async () => {
      try {
        const t = await ensureStandingThread(bot.id, bot.name);
        if (cancelled) return;
        setThread(t);
        if (!t.currentSessionId) return;
        void getSessionPaused(t.currentSessionId).then(p => !cancelled && setThreadPaused(p)).catch(() => {});
        const messages = await listSessionMessages(t.currentSessionId);
        if (cancelled) return;
        setThreadLines(hudLinesFromMessages(messages));
      } catch (err) {
        if (!cancelled) setError(`Couldn't open ${bot.name}'s thread: ${errorMessage(err)}`);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [bot.id, bot.name, online]);

  // A new bot means a new incognito conversation.
  useEffect(() => {
    incognitoThread.current = null;
    setIncognitoLines([]);
    setIncognitoPaused(null);
  }, [bot.id]);

  useEffect(() => {
    setBotCursor(Math.max(0, bots.findIndex(b => b.id === currentId)));
  }, [bots, currentId, setBotCursor]);

  const close = useCallback(() => {
    inFlight.current?.abort();
    // An incognito ask ends with the HUD; nothing about it is kept.
    const incognito = incognitoThread.current;
    if (incognito) void threadApi.resolve(incognito.id, 'done').catch(() => {});
    onDone(undefined, { display: 'skip' });
  }, [onDone]);

  const send = useCallback(async (raw: string) => {
    const text = raw.trim();
    if (!text || busy) return;
    setInput('');
    setCursor(0);
    setError(null);
    setScroll(0);
    const controller = new AbortController();
    inFlight.current = controller;
    setBusy(true);
    const mine: ChatLine = { role: 'user', content: text };
    try {
      if (tabRef.current === 'thread') {
        if (!thread) return;
        setThreadLines(lines => [...lines, mine]);
        const reply = await sendThreadTurn(thread, text, { model: bot.model, signal: controller.signal });
        setThreadLines(lines => [...lines, { role: 'assistant', content: reply.content }]);
        const next = await followThread(thread, {
          tokensUsed: turnContextTokens(reply),
          model: bot.model ? `${bot.model.providerID}/${bot.model.modelID}` : undefined,
        });
        // A handoff moved the thread to a fresh window: mark it where it happened.
        if (next.currentSessionId && next.currentSessionId !== thread.currentSessionId) {
          setThreadLines(lines => [...lines, { role: 'rip', generation: next.generation, reason: 'threshold', at: new Date().toISOString(), from: thread.currentSessionId ?? undefined }]);
        }
        setThread(next);
        if (next.currentSessionId) setThreadPaused(await getSessionPaused(next.currentSessionId).catch(() => null));
      } else {
        const history = [...incognitoLines, mine];
        setIncognitoLines(history);
        let answer: string | null;
        if (online) {
          incognitoThread.current ??= await createIncognitoThread(bot.id, `Incognito ask: ${text.slice(0, 60)}`);
          answer = (await sendThreadTurn(incognitoThread.current, text, { model: bot.model, signal: controller.signal })).content;
          const sid = incognitoThread.current.currentSessionId;
          if (sid) setIncognitoPaused(await getSessionPaused(sid).catch(() => null));
        } else {
          const timer = setTimeout(() => controller.abort(), LOCAL_TIMEOUT_MS);
          try {
            answer = await askPetLocally(history, bot, controller.signal);
          } finally {
            clearTimeout(timer);
          }
          if (!answer && !controller.signal.aborted) throw new Error("gizzi's model didn't answer. Check `/model`.");
        }
        if (answer) setIncognitoLines(lines => [...lines, { role: 'assistant', content: answer! }]);
      }
    } catch (err) {
      if (!controller.signal.aborted) {
        setError(err instanceof PlatformSignedOutError ? 'Signed out. Run `gizzi login` to use your bots.' : errorMessage(err));
      }
    } finally {
      if (inFlight.current === controller) inFlight.current = null;
      setBusy(false);
    }
  }, [busy, tab, thread, bot, online, incognitoLines]);

  // ↑ at the top of the input scrolls the thread back; past the oldest line,
  // a rip's earlier window is read in place (the same messages Desktop
  // shows under "Show the earlier conversation").
  const scrollUp = useCallback(() => {
    if (tabRef.current !== 'thread' || earlier.state === 'loading') return;
    if (scroll < maxOffset(threadLines, SHOWN_LINES)) {
      setScroll(o => o + 1);
      return;
    }
    const from = earlierWindowAtTop(threadLines, earlier.loaded);
    if (!from) return;
    setEarlier(e => ({ ...e, state: 'loading' }));
    void listSessionMessages(from).then(messages => {
      const older = hudLinesFromMessages(messages);
      setThreadLines(lines => [...older, ...lines]);
      if (older.length > 0) setScroll(o => o + 1);
      setEarlier(e => ({ loaded: new Set(e.loaded).add(from), state: 'idle' }));
    }, () => setEarlier(e => ({ ...e, state: 'error' })));
  }, [scroll, threadLines, earlier]);
  const scrollDown = useCallback(() => {
    if (tabRef.current === 'thread') setScroll(o => Math.max(0, o - 1));
  }, []);

  useInput((_input, key) => {
    if (key.escape) {
      close();
      return;
    }
    const current = tabRef.current;
    if (key.tab) {
      const i = TABS.findIndex(t => t.id === current);
      setTab(TABS[(i + (key.shift ? TABS.length - 1 : 1)) % TABS.length]!.id);
      setError(null);
      return;
    }
    if (current !== 'bots') return;
    if (key.upArrow) setBotCursor(c => Math.max(0, c - 1));
    if (key.downArrow) setBotCursor(c => Math.min(bots.length - 1, c + 1));
    const picked = bots[botCursorRef.current];
    if (key.return && picked) {
      selectPetBot(picked.id);
      setTab('thread');
    }
  });

  const incognito = tab === 'incognito';
  const signInHint = expired ? 'Your Allternit sign-in was rejected. Run `gizzi login` to sign in again (approve it in Allternit Desktop).' : 'Run `gizzi login` and approve it in Allternit Desktop to use your bots.';
  const status = tab === 'incognito'
    ? online ? 'not saved' : 'not saved · on this Mac'
    : connection === 'connecting' ? 'connecting…'
    : connection === 'signed-out' ? 'signed out'
    : connection === 'offline' ? 'offline'
    : tab === 'bots' ? 'your Allternit bots' : thread ? 'thread · shared with Desktop' : 'opening thread…';

  let body: React.ReactNode;
  if (tab === 'bots') {
    body = <Box flexDirection="column">
        {bots.map((b, i) => <Box key={b.id} flexDirection="row">
            <Text color={accent}>{i === botCursor ? '› ' : '  '}</Text>
            <Text bold={b.id === currentId}>{b.name}</Text>
            <Text dimColor>{'  '}{b.id === currentId ? 'wearing · ' : ''}{b.description}</Text>
          </Box>)}
        {loading && <Text dimColor>Loading your bots…</Text>}
        {connection === 'signed-out' && <Text dimColor>{signInHint}</Text>}
        {connection === 'offline' && <Text color="warning">Couldn't reach Allternit, so only Gizzi is listed.</Text>}
      </Box>;
  } else if (tab === 'thread' && !online) {
    body = <Text dimColor wrap="wrap">
        {connection === 'connecting' ? `Connecting to Allternit…` : connection === 'signed-out' ? `${bot.name}'s thread lives on Allternit. ${signInHint}` : `Couldn't reach Allternit to open ${bot.name}'s thread.`}
        {connection !== 'connecting' ? ' Tab to Incognito for a quick question on this Mac.' : ''}
      </Text>;
  } else {
    const lines = incognito ? incognitoLines : threadLines;
    const ready = incognito || !!thread;
    body = <Box flexDirection="column">
        {lines.length > 0 ? <Lines lines={lines} offset={incognito ? 0 : scroll} bot={bot} accent={accent} width={Math.max(30, columns - 20)} /> : <Text dimColor>
            {incognito ? `Ask ${bot.name} something. Nothing here is saved.` : ready ? `Start ${bot.name}'s thread.` : `Opening ${bot.name}'s thread…`}
          </Text>}
        {!incognito && earlier.state !== 'idle' && <Text dimColor={earlier.state === 'loading'} color={earlier.state === 'error' ? 'warning' : undefined}>
            {earlier.state === 'loading' ? 'Loading the earlier conversation…' : "Couldn't load the earlier conversation. ↑ to try again."}
          </Text>}
        {!incognito && scroll > 0 && <Text dimColor>{`↓ ${scroll} newer`}</Text>}
        {(incognito ? incognitoPaused : threadPaused) && <Text color="warning" wrap="wrap">
            {'⏸ '}{pausedLine((incognito ? incognitoPaused : threadPaused)!)}
          </Text>}
        {busy && <Box flexDirection="row">
            <SpinnerGlyph frame={frame} messageColor="gizzi" />
            <Text dimColor>{bot.name} is thinking…</Text>
          </Box>}
        {ready && <Box flexDirection="row">
            <Text color={accent}>{'> '}</Text>
            <TextInput value={input} onChange={setInput} onSubmit={value => void send(value)} onHistoryUp={scrollUp} onHistoryDown={scrollDown} columns={Math.max(20, columns - 12)} cursorOffset={cursor} onChangeCursorOffset={setCursor} focus={!busy} showCursor placeholder={`Ask ${bot.name}…`} />
          </Box>}
      </Box>;
  }

  const canScrollBack = scroll < maxOffset(threadLines, SHOWN_LINES) || !!earlierWindowAtTop(threadLines, earlier.loaded);
  const hint = tab === 'bots'
    ? '↑↓ choose · Enter wear · Tab thread · Esc close'
    : incognito ? 'Enter send · Tab bots · Esc close and forget'
    : `Enter send · ${canScrollBack ? '↑ earlier · ' : ''}Tab incognito · Esc close`;

  return <Box flexDirection="column" borderStyle={incognito ? 'dashed' : 'round'} borderColor={incognito ? 'inactive' : accent} paddingX={1} marginTop={1}>
      <Box flexDirection="row" justifyContent="space-between">
        <Box flexDirection="row">
          <Text bold color={accent}>{bot.name}</Text>
          {TABS.map(t => <Text key={t.id} bold={t.id === tab} dimColor={t.id !== tab}>
              {'  '}{t.id === tab ? `[${t.label}]` : t.label}
            </Text>)}
        </Box>
        <Text dimColor>{status}</Text>
      </Box>
      {body}
      {error && <Text color="error" wrap="wrap">{error}</Text>}
      <Text dimColor>{hint}</Text>
    </Box>;
}
