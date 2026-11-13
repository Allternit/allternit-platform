// Allternit Meetings: platform adapter interface and pod config (step 1.1).
// Frozen by docs/meetings/CONTRACT.md. One adapter per platform (meet, zoom, teams).
// Built from what spike 0.3 proved in spike/join.ts: prejoin -> join click ->
// waiting_admit -> live, chat poll, active-speaker poll at 250 ms, participant
// set, a Meet refusal reported as guest_blocked, and a decline that is final.
// No imports on purpose: adapters bring their own driver (playwright-core, Zoom SDK sidecar).

export type Platform = "meet" | "zoom" | "teams";

/** Tile states, same set as spike/tile-feed.ts TILE_STATES. */
export type TileState = "joining" | "listening" | "thinking" | "speaking" | "paused";

export type EndReason =
  | "left_by_user"
  | "removed"
  | "not_admitted"
  | "empty_room"
  | "max_duration"
  | "other_notetaker"
  | "join_failed"
  | "guest_blocked"
  | "no_capacity"
  | "error";

/** Pod-side states an adapter reports while joining (maps to session states in CONTRACT.md). */
export type JoinPhase = "prejoin" | "join_clicked" | "waiting_admit" | "live";

export type Unsubscribe = () => void;

// ---- identity -----------------------------------------------------------------

/**
 * How the bot appears in the call.
 * - google_account: a signed-in bot Google account (Meet refuses guests on free-Gmail calls).
 * - zoom_obf: Meeting SDK joined with an on-behalf-of token from the user's connected Zoom account.
 * - anonymous: Teams anonymous web join with the display name.
 */
export type BotIdentity =
  | { kind: "google_account"; accountEmail: string; profileDir: string }
  | { kind: "zoom_obf"; obfToken: string; sdkKey: string; sdkSecretRef: string }
  | { kind: "anonymous" };

// ---- join ----------------------------------------------------------------------

export interface JoinRequest {
  /** Canonical join URL from the meetings row. */
  url: string;
  /** Display name, "Gizzi (Allternit)". */
  displayName: string;
  identity: BotIdentity;
  /** Give up waiting to be admitted after this long (contract: 10 min). */
  admitTimeoutMs: number;
  /** Give up on the pre-join screen after this long (spike: 90 s). */
  prejoinTimeoutMs: number;
}

export type JoinFailureCode = "guest_blocked" | "join_failed" | "error";

export interface JoinOk {
  ok: true;
  /** Epoch ms when the bot became live in the call. */
  liveAt: number;
}
export interface JoinFailure {
  ok: false;
  /** Always an EndReason the runner can pass straight to the session. */
  reason: Extract<EndReason, "not_admitted" | "guest_blocked" | "join_failed" | "error">;
  detail?: string;
  /** Path to a screenshot of the failing screen, if one was taken. */
  screenshot?: string;
}
export type JoinResult = JoinOk | JoinFailure;

export interface AdmitResult {
  admitted: boolean;
  /** Set when not admitted. A decline is final: the runner never retries. */
  reason?: Extract<EndReason, "not_admitted" | "guest_blocked" | "join_failed" | "error">;
}

// ---- in-call data -----------------------------------------------------------------

export interface ChatMessage {
  /** Platform message id, or a stable hash when the platform gives none. Used as the ingest idempotency key. */
  id: string;
  sender: string;
  text: string;
  /** Epoch ms the adapter first saw it. */
  t: number;
  /** True for messages the bot itself posted (adapters filter these from onChat by default). */
  self: boolean;
}

export interface Participant {
  /** Platform participant id (Meet data-participant-id), or the name when none. */
  id: string;
  name: string;
  email?: string;
  /** Platform-reported: signed in to the host's organisation. Unknown when the platform does not say. */
  internal?: boolean;
}

export interface ActiveSpeakerEvent {
  /** Epoch ms of the poll. */
  t: number;
  /** Names currently speaking (empty = silence). */
  names: string[];
}

export interface CaptionEvent {
  t: number;
  speaker: string;
  text: string;
  /** False while the platform may still revise the line. */
  final: boolean;
}

// ---- adapter -----------------------------------------------------------------------

export interface PlatformAdapter {
  readonly platform: Platform;

  /**
   * Open the call, pass pre-join, click join, and wait until live or failed.
   * Reports each phase through onPhase. Resolves with a JoinResult; never throws for
   * expected failures (blocked, declined, timeout). One call from the runner's view; an
   * adapter may retry internally within admitTimeoutMs (Zoom: while the authorizing user is
   * not yet in the meeting), reporting waiting_admit. A host decline is final, never retried.
   */
  join(req: JoinRequest, onPhase?: (phase: JoinPhase) => void): Promise<JoinResult>;

  /** Wait for the host to admit the bot. join() already awaits this; exposed so tests can drive it. */
  waitForAdmit(timeoutMs: number): Promise<AdmitResult>;

  /** Post a message in the meeting chat. Resolves false if the platform refused (e.g. anonymous Teams chat). */
  postChat(text: string): Promise<boolean>;

  /** New chat messages from others, polled about every second. */
  onChat(cb: (m: ChatMessage) => void): Unsubscribe;

  /** Current participant set (excluding the bot). */
  participants(): Promise<Participant[]>;
  /** Set changes, with the full new set. */
  onParticipants(cb: (all: Participant[]) => void): Unsubscribe;

  /** Active-speaker poll, about every 250 ms. Feeds the speaker timeline merged with STT segments. */
  onActiveSpeaker(cb: (e: ActiveSpeakerEvent) => void): Unsubscribe;

  /** Platform live captions, a cross-check on speaker labels. Resolves false if captions cannot be turned on. */
  captions(cb: (e: CaptionEvent) => void): Promise<boolean>;

  /** Update the camera tile (Bot Mode stage) state. */
  setTile(state: TileState): Promise<void>;

  /** Leave the call. Idempotent. */
  leave(): Promise<void>;

  /** Resolves when the call ends on its own (removed, meeting closed, empty room is detected by the runner). */
  onEnded(cb: (reason: Extract<EndReason, "removed" | "left_by_user" | "error">) => void): Unsubscribe;

  /** Tear down the browser or sidecar. Idempotent. */
  close(): Promise<void>;
}

export type AdapterFactory = (cfg: PodConfig) => PlatformAdapter;

// ---- pod config --------------------------------------------------------------------

/** Environment-derived config the cloud-api launcher gives each pod (one pod per meeting session). */
export interface PodConfig {
  sessionId: string;
  meetingId: string;
  platform: Platform;
  joinUrl: string;
  displayName: string;
  identity: BotIdentity;
  /** Base URL of cloud-api, e.g. https://api.allternit.com. */
  cloudApiUrl: string;
  /** Pod token (bearer) accepted only by /meeting-sessions/:sid/ingest, /tool-calls and the control stream. */
  podToken: string;
  /** LiveKit room, "meeting-<sid>", with a publish/subscribe token for the pod. */
  livekit: { url: string; room: string; token: string };
  /** Bot Mode stage page captured into the camera (allternit-ai /meeting-tile). */
  tileUrl: string;
  /** Replies as meeting chat (default) or also spoken. Live-switchable by control messages. */
  speakReplies: boolean;
  /** Guest-quiet: post only the notice, replies go to the page. */
  guestQuiet: boolean;
  /** Notice text posted after admit. */
  noticeText: string;
  limits: {
    admitTimeoutMs: number; // 10 min
    prejoinTimeoutMs: number; // 90 s
    emptyRoomMs: number; // 5 min
    maxDurationMs: number; // 8 h
    screenNoteMinIntervalMs: number;
  };
}
