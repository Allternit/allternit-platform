/** Typed errors for the AAI REST surface. Server error body: { error, code, retryAfterMs, approvalId }. */
export class AaiHttpError extends Error {
  constructor(
    public readonly status: number,
    public readonly code: string | undefined,
    message: string,
    public readonly body: unknown,
    public readonly retryAfterMs?: number,
  ) {
    super(message);
    this.name = "AaiHttpError";
  }
}

/** 428 APPROVAL_REQUIRED: a human must approve `approvalId` before the turn is sent to the vendor. */
export class ApprovalRequiredError extends AaiHttpError {
  constructor(message: string, body: unknown, public readonly approvalId: string | undefined) {
    super(428, "APPROVAL_REQUIRED", message, body);
    this.name = "ApprovalRequiredError";
  }
}

/** 409: BINDING_NOT_READY, REMOTE_CLOSED, CONTEXT_LOST(_RESUMABLE), sign-in needed, ALREADY_RESOLVED, ... */
export class ConflictError extends AaiHttpError {
  constructor(code: string | undefined, message: string, body: unknown, public readonly approvalId?: string) {
    super(409, code, message, body);
    this.name = "ConflictError";
  }
}

export class RateLimitedError extends AaiHttpError {
  constructor(message: string, body: unknown, retryAfterMs?: number) {
    super(429, "RATE_LIMITED", message, body, retryAfterMs);
    this.name = "RateLimitedError";
  }
}

/** Approvals can only be answered by a person; the SDK refuses without explicit human intent. */
export class HumanIntentRequiredError extends Error {
  constructor() {
    super("respondApproval requires { humanIntent: true }: approvals are answered by a person, never automated");
    this.name = "HumanIntentRequiredError";
  }
}

export function toHttpError(status: number, body: any): AaiHttpError {
  const message = String(body?.error ?? body?.message ?? `HTTP ${status}`);
  const code = typeof body?.code === "string" ? body.code : undefined;
  const approvalId = typeof body?.approvalId === "string" ? body.approvalId : undefined;
  if (status === 428) return new ApprovalRequiredError(message, body, approvalId);
  if (status === 409) return new ConflictError(code, message, body, approvalId);
  if (status === 429) return new RateLimitedError(message, body, typeof body?.retryAfterMs === "number" ? body.retryAfterMs : undefined);
  return new AaiHttpError(status, code, message, body, typeof body?.retryAfterMs === "number" ? body.retryAfterMs : undefined);
}
