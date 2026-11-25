/** Ports that are never shared: allternit-api (prod and dev) and the gizzi runtime. The gate enforces too (D7). */
export const DENIED_PORTS: readonly number[] = [8013, 18013, 4096];

/** The gate only knows `token` (visitor sends the per-share token) and `public` (apps only). Cloud-api's `signed_in`/`org` links map to `token`. */
export type AuthMode = "token" | "public";

export type ShareSubject = "app" | "mcp";

/** Why a live link stopped. `killed`, `revoked` and `not_found` come from cloud-api's heartbeat. */
export type StopReason =
  | "stopped"
  | "paused"
  | "killed"
  | "revoked"
  | "not_found"
  | "relay_closed"
  | "error";

export type ShareTunnelErrorCode =
  | "port_denied"
  | "bad_port"
  | "admission_required"
  | "admission_refused"
  | "name_mismatch"
  | "gate_failed"
  | "relay_failed";

export class ShareTunnelError extends Error {
  constructor(
    readonly code: ShareTunnelErrorCode,
    message: string,
    override readonly cause?: unknown,
  ) {
    super(message);
    this.name = "ShareTunnelError";
  }
}

/** One local gate (allternit-api `POST /shares`): a loopback listener forwarding to the one shared port. */
export interface GateShare {
  /** `shr_...` */
  id: string;
  /** `127.0.0.1:<gate_port>`: the relay route target. */
  target: string;
  /** `ast_...`, shown once (create and rotate). Visitors send it as `X-Allternit-Share-Token`, never Bearer. */
  token: string;
  authMode: AuthMode;
  paused: boolean;
}

export interface ShareGate {
  open(input: {
    /** cloud-api share link id, sent as `share_link_id`. */
    linkId: string;
    port: number;
    authMode: AuthMode;
    subject?: ShareSubject;
    label?: string;
  }): Promise<GateShare>;
  /** Stops the gate (`POST /shares/:id/stop`); an unknown id is not an error. */
  close(id: string): Promise<void>;
  /** Gate answers 503 `share_paused` while paused. Returns the new paused state. */
  pause?(id: string, paused?: boolean): Promise<boolean>;
  /** Invalidates the old token; returns the new one. */
  rotate?(id: string): Promise<string>;
}

export interface StartShareInput {
  /** The cloud-api share link (`POST /api/v1/share-links`, kind `live`): its id and subdomain. */
  link: { id: string; subdomain: string };
  /** Local port to share. Refused when it is in `DENIED_PORTS` (or the options' extra list). */
  port: number;
  /** Default `"token"`. `"public"` is refused by the gate for MCP shares. */
  authMode?: AuthMode;
  /** Default `"app"`. */
  subject?: ShareSubject;
  label?: string;
  /**
   * The relay admission credential cloud-api mints for this link (`relay_credential` on create, or
   * `POST /api/v1/share-links/:id/relay-credential`). Needed only the first time a link is served on this
   * device; later starts reuse the saved tunnel identity. A function is called only when needed.
   */
  admission?: string | (() => Promise<string>);
}

export interface LiveShareStatus {
  state: "connected" | "reconnecting" | "stopped";
  /** Open visitor connections right now. */
  connections: number;
  lastHeartbeatAt?: number;
  lastError?: string;
  stopReason?: StopReason;
}

export type ShareTunnelEvent =
  | { type: "started"; linkId: string; url: string }
  | { type: "reconnecting"; linkId: string; attempt: number }
  | { type: "connected"; linkId: string }
  | { type: "heartbeat"; linkId: string; ok: boolean; status?: number; error?: string }
  | { type: "stopped"; linkId: string; reason: StopReason; error?: string };

export interface LiveShare {
  readonly linkId: string;
  readonly subdomain: string;
  /** `https://<relay hostname>`, e.g. `https://<subdomain>.live.allternitusercontent.com`. */
  readonly url: string;
  readonly port: number;
  /** Per-share gate token (`ast_...`); put it in the visitor link or send `X-Allternit-Share-Token`. Changes on `rotate()`. */
  readonly token: string;
  /** Pauses or resumes the local gate (visitors get 503 `share_paused`); the tunnel stays up. */
  pause(paused?: boolean): Promise<void>;
  /** New gate token; the old one stops working. Resolves with the new token. */
  rotate(): Promise<string>;
  status(): LiveShareStatus;
  /** Stops serving; idempotent. Resolves with the first stop reason. */
  stop(reason?: StopReason): Promise<StopReason>;
  /** Settles once with the stop reason, whoever stopped it. */
  readonly stopped: Promise<StopReason>;
}
