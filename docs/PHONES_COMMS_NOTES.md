# Phones 2.0 — Phase 3 user-comms core: lane notes

Lane: user comms (Desktop main). Branch `session/phones-comms`, worktree `allternit/allternit-wt-phones-comms`.
Date: 2026-10-17. Base: `de93fa9558` (origin/main minus one merge — lane rule: no pull/rebase).

## Files created (the only files touched)

- `surfaces/allternit-desktop/src/main/phone-user-comms.ts` — the class + pure exported parsers/helpers.
- `surfaces/allternit-desktop/src/main/phone-user-comms.test.ts` — fixture-driven tests (58 tests).
- `docs/PHONES_COMMS_NOTES.md`, `docs/PHONES_COMMS.sentinel` — this file.

Sibling-owned files (`phone-ipc.ts`, `preload/**`, `phone-service.ts`, `phone-device-manager.ts`,
`phone-scrcpy.ts`, `phone-stream.ts`, `phone-link*.ts`) were **not** touched. The class imports
(never modifies) `phone-tools.js` for `buildIsmsSend`, `checkRate`, `isEmergencyNumber`,
`normalizeNumber`, `shQuote` and the `AdbCall`/`RateStore` types — the injected runner shape is
identical to `PhoneToolDeps.adb`, so wiring is mechanical.

## Class API (exact exported signatures)

```ts
export class PhoneUserComms {
  constructor(deps: PhoneUserCommsDeps);

  // SMS read
  smsList(serial: string): Promise<CommsResult<{ threads: PhoneSmsThread[] }>>;
  smsMessages(serial: string, address: string, opts?: { afterTimestamp?: number }):
    Promise<CommsResult<{ messages: PhoneSmsMessage[] }>>;

  // SMS send (user lane)
  smsSendUser(serial: string, to: string, text: string):
    Promise<CommsResult<{ status: 'submitted'; emergency?: true }>>;

  // Calls (user lane)
  callDialUser(serial: string, number: string, opts?: { confirmEmergency?: boolean }):
    Promise<CommsResult<{ status: 'dialing'; emergency?: true }>>;
  callEnd(serial: string): Promise<CommsResult<{}>>;
  callState(serial: string): Promise<CommsResult<{ state: CallStateName; number?: string }>>;

  // SMS watcher (bridge name `onCommsEvent` is an alias of `onSmsEvent`)
  onSmsEvent(cb: (ev: CommsEvent) => void): () => void;
  onCommsEvent(cb: (ev: CommsEvent) => void): () => void;
  get watcherActive(): boolean;

  // App inventory
  listApps(serial: string): Promise<CommsResult<{ apps: PhoneAppInfo[] }>>;
  invalidateApps(serial?: string): void;
}

export type CommsResult<T> = ({ ok: true } & T) | { ok: false; message: string; retryAfterSec?: number };
export type CallStateName = 'idle' | 'dialing' | 'active';
export type CommsEvent = { type: 'sms'; address: string };
export interface PhoneSmsThread { address: string; displayName?: string; lastSnippet: string; lastTimestamp: number; unreadCount?: number }
export interface PhoneSmsMessage { id: string; direction: 'in' | 'out'; body: string; timestamp: number }
export interface PhoneAppInfo { pkg: string; versionName?: string }

export interface PhoneUserCommsDeps {
  adb: AdbCall;                                        // same shape as PhoneToolDeps.adb
  rates: RateStore;                                    // same shape as the bot lane's store
  onlineSerials: () => string[];                       // watcher polls all of these
  now?: () => number;                                  // injectable clock (ms)
  pollIntervalMs?: number;                             // default 15_000
  appCacheTtlMs?: number;                              // default 600_000
  maxAppConcurrency?: number;                          // default 8
  log?: (message: string) => void;                     // swallowed background errors
}

export const USER_SMS_LIMITS = { minGapMs: 5_000, perHour: Number.POSITIVE_INFINITY, perDay: 100 } as const;
// Also exported for reuse/tests: DEFAULT_POLL_INTERVAL_MS, APP_CACHE_TTL_MS, MAX_APP_CONCURRENCY,
// normalizeAddress, parseContentQueryRows, buildThreads, buildMessages, messageId,
// parsePackageList, parseVersionName, parseCallState, mapWithConcurrency, RawSmsRow.
```

Wiring sketch for the orchestrator (IPC handler bodies are one-liners): deps are
`adb` = the same lazy adb call `phone-service.ts` builds for the tool runner
(`ensureAdbCall()`), `rates` = `fileRateStore(.../rates.json)` (user sends key on
`${serial}:sms:user` — never collides with the bot lane's `${serial}:sms`),
`onlineSerials` = `() => manager?.onlineSerials() ?? []`, `now`/`pollIntervalMs` defaults.
The IPC layer owns serial resolution (the bridge carries no serial — resolve like the other
phone IPC handlers do) and any dialogs.

## Contract alignment (pinned PhoneCommsBridge from the home-lane task)

Aligned 1:1: `smsList` / `smsMessages` / `smsSendUser` / `callDialUser` / `callEnd` /
`callState` / `onCommsEvent`. Deviations, all additive (supersets — a consumer reading only the
pinned fields sees exactly the pinned shape):

- `callDialUser(number, { confirmEmergency })`: per the home-lane spec, "Desktop main refuses
  without `confirmEmergency: true`" — implemented in the class (`{ ok: false, message:
  'emergency_confirm_required' }`, adb never touched). `smsSendUser` has no such refusal: the
  pinned bridge shape has no opts there, and this lane's task says send + flag. Emergency texts
  ARE sent and come back `{ ok: true, status: 'submitted', emergency: true }`.
- Success results carry `status: 'submitted'` / `'dialing'` — never 'delivered' (SMS delivery is
  not confirmable via `service call isms`; call audio stays on the physical phone).
- `callState` returns `CommsResult` (`{ ok: false, message }` when both dumpsys fail). The pinned
  bridge type has no `ok` — the IPC adapter should map `ok:false` to `{ state: 'idle' }` (or cache
  the last good state); document this choice at wiring time.
- `PhoneSmsMessage.id` is synthesized (`direction:timestamp:fnv1a(address+body)`) because the
  pinned projection (`address:body:date:read`) has no `_id`. Deterministic and stable across
  polls while content is unchanged; fine for React keys. If message-identity bugs ever matter,
  widen the projection to `_id:address:body:date:read` and use `_id` — the parser splits on known
  keys, so it will keep working with one added key.
- `PhoneSmsThread.displayName` is left `undefined` (hook only) — contact resolution is the UI's job.

## Parsing assumptions (adb output is raw, never escaped)

- `content query --projection address:body:date:read` prints `Row: N address=A, body=B, date=D,
  read=R` with values verbatim. A body containing commas/colons/newlines lands raw; a newline
  physically breaks the row across lines. Parser: known-keys-only splitting from the ends inward
  — address = first `address=`…first `, body=`; tail = the LAST `, date=<9–16 digits>, read=0|1`
  that validates as the row suffix (a decoy `, date=…` inside a body is skipped by the backward
  scan — fixture-tested); body = middle slice, embedded `\n`/commas/colons preserved. Non-Row
  stray lines before the first Row are ignored; a line that isn't a continuation and isn't a Row
  starts nothing. Malformed Row lines are counted and skipped.
- Empty box: `content query` prints nothing → `{ ok: true, threads: [] }`. Non-empty stdout with
  zero parseable Rows → `parse_failed`.
- OEM block: `SecurityException` / `Permission Denial` / "not allowed" / "Unable to find provider"
  (or a non-zero exit whose output matches Exception/Permission/denied) → `{ ok: false, message:
  'read_blocked' }`, distinctly from empty. A plain transport failure (`error: device offline`)
  stays `adb_failed` — do not merge the two in the UI copy.
- `date` is epoch **milliseconds**; `afterTimestamp` is exclusive (`>`).
- Address normalization strips only `[\s().-]` (keeps leading `+`, shortcodes, alphanumerics);
  threads key on the normalized form, so "+1 (555) 123-4567" merges with "+15551234567".
- `pm list packages -3` → one `package:<pkg>` per line; any non-package line → `parse_failed`.
  `dumpsys package <pkg>` → first indented `versionName=` line; per-package failure →
  `versionName` omitted (list still succeeds). Labels are not available via adb — the UI maps
  known packages (per the task).
- siminfo subId: `_id` of the `sim_id=0` row (else first populated slot); unreadable → subId `0`
  = `DEFAULT_SUBSCRIPTION_ID` (the platform default SIM — the "no explicit subscriber" behavior;
  `buildIsmsSend` always carries the i32 subId, so 0 is the faithful fallback).

## callState heuristic — limits (also in the file header)

Sources: `dumpsys activity activities | grep -i incall` + `dumpsys telephony.registry | grep -i
callState`. Mapping: any `mCallState=1|2` → live (1 = ringing maps to 'active' — the 3-state
contract has no 'ringing'); `2` (offhook) + in-call UI visible → 'active'; offhook + no UI match
yet → 'dialing'; all-zero + no UI → 'idle'. Number: `mCallIncomingNumber=` else `dat=tel:` in the
activities dump, else omitted.

Limits: (1) once the OEM in-call UI is foreground (which includes the "Calling…" window on
Pixel/Samsung) adb cannot see "connected vs ringing vs dialing" — 'dialing' only appears in the
brief pre-UI window or when the call screen doesn't match `/incall/i` (car mode, some OEM skins);
(2) OEMs that rename the telephony-registry fields fall back to UI-presence (incall → 'active');
(3) the number is whatever the dumps happen to reveal. Treat it as a UI hint, not ground truth;
the UI polls every 2 s while a call card is up, which absorbs the races.

## Test output summary

- New suite `src/main/phone-user-comms.test.ts`: **58/58 pass** (~0.8 s). Fixture-driven, no
  device: multi-line/comma/colon/unicode/decoy-tail Row fixtures, OEM-block paths, injectable
  clock for the 5 s gap and 100/day cap, bot-lane key isolation, failed-send quota parity,
  emergency flag + `confirmEmergency` gate, all callState mappings + OEM fallback, watcher
  baseline/emit/pause/unsubscribe/throwing-listener/multi-serial, listApps concurrency bound
  (≤8 in flight, verified >1), 10-min cache + invalidation.
- Full desktop main suite: **69 files / 583 tests, all pass** (`pnpm test`, vitest).
- `pnpm typecheck` (main + preload): **exit 0**.
- Dependency-map impact (`scripts/dependency-map.py --impact … --summary`): part of Allternit
  Desktop; touches feature **"Bots operate real phones" [live]**; reaches the Desktop app +
  ai.allternit.com + m.allternit.com PWA through shared code. features.json was not edited —
  the feature/component bookkeeping for the Phase-3 landing belongs to the merge orchestration,
  not this lane (its warnings about stale evidence paths are pre-existing on main).

## Honest deferrals

- No IPC/preload wiring (owned by the orchestrator; this class is the entire surface to wire).
- The `callState` 'dialing' ambiguity above is a platform limitation, not a TODO.
- `smsMessages` re-queries both boxes per call; the UI's 2 s poll + 15 s watcher are well inside
  adb's comfort, but a future `content://sms` (all boxes) single query with a `type` column would
  halve the round-trips — deliberately not deviated from the pinned projection.
