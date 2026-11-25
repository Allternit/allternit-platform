/** Ports that are never shared: allternit-api (prod and dev) and the gizzi runtime. The gate enforces too (D7). */
export const DENIED_PORTS: readonly number[] = [8013, 18013, 4096];

export type AuthMode = "public" | "signed_in" | "org";

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
    readonly cause?: unknown,
  ) {
    super(message);
    this.name = "ShareTunnelError";
  }
}

/** The local share gate in allternit-api (D2): forwards relay traffic to one local port, with link auth. */
export interface ShareGate {
  open(input: {
    linkId: string;
    subdomain: string;
    port: number;
    authMode: AuthMode;
  }): Promise<{ id: string; target: string }>;
  close(id: string): Promise<void>;
}

export interface StartShareInput {
  /** The cloud-api share link (`POST /api/v1/share-links`, kind `live`): its id and subdomain. */
  link: { id: string; subdomain: string };
  /** Local port to share. Refused when it is in `DENIED_PORTS` (or the options' extra list). */
  port: number;
  authMode?: AuthMode;
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
  status(): LiveShareStatus;
  /** Stops serving; idempotent. Resolves with the first stop reason. */
  stop(reason?: StopReason): Promise<StopReason>;
  /** Settles once with the stop reason, whoever stopped it. */
  readonly stopped: Promise<StopReason>;
}
