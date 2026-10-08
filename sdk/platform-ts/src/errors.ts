/**
 * Typed errors. Every non-2xx answer from the API becomes an `APIError`
 * subclass picked by HTTP status, carrying the API's
 * `{ error: { type, code, message, param } }` fields.
 */

export interface ErrorBody {
  type?: string;
  code?: string;
  message?: string;
  param?: string | null;
  /** Where to fix it (402 `payment_method_required`: the console billing page). */
  url?: string;
}

export class AllternitError extends Error {
  constructor(message: string) {
    super(message);
    this.name = new.target.name;
  }
}

export class APIError extends AllternitError {
  /** HTTP status (0 for an `error` event inside a stream that had already started). */
  readonly status: number;
  /** API error type, e.g. `invalid_request_error`. */
  readonly type: string | undefined;
  /** Machine-readable code, e.g. `agent_not_found`. */
  readonly code: string | undefined;
  /** The request field the error is about, if any. */
  readonly param: string | null | undefined;
  /** Where to fix it, when the API says (402 `payment_method_required`: the console billing page). */
  readonly url: string | undefined;
  /** `x-request-id` response header, when the server sent one. */
  readonly requestId: string | undefined;
  readonly headers: Headers | undefined;

  constructor(status: number, body: ErrorBody | undefined, headers?: Headers, fallback?: string) {
    super(body?.message || fallback || `HTTP ${status}`);
    this.status = status;
    this.type = body?.type;
    this.code = body?.code;
    this.param = body?.param;
    this.url = body?.url;
    this.headers = headers;
    this.requestId = headers?.get("x-request-id") ?? undefined;
  }
}

/** 400 / 422: the request was malformed or a field is invalid. */
export class InvalidRequestError extends APIError {}
/** 401: missing, invalid or revoked API key. */
export class AuthenticationError extends APIError {}
/**
 * 402 `billing_error`: `payment_method_required` (no card on file: open `url`,
 * the console billing page) or `spend_cap_reached` (raise the cap in the console).
 */
export class PaymentRequiredError extends APIError {}
/** 403: the key lacks a scope, or is bound to another account. */
export class PermissionError extends APIError {}
/** 404: no such resource (or it belongs to another project). */
export class NotFoundError extends APIError {}
/** 409: conflict, e.g. `conversation_busy` or an idempotency key reused with a different request. */
export class ConflictError extends APIError {}
/** 429: rate limited. `retryAfter` is in seconds when the server sent `Retry-After`. */
export class RateLimitError extends APIError {
  readonly retryAfter: number | undefined;
  constructor(status: number, body: ErrorBody | undefined, headers?: Headers, fallback?: string) {
    super(status, body, headers, fallback);
    const raw = headers?.get("retry-after");
    const n = raw == null ? NaN : Number(raw);
    this.retryAfter = Number.isFinite(n) ? n : undefined;
  }
}
/** 5xx: the API failed, or (`code: "runtime_starting"`) asks you to retry shortly. */
export class InternalServerError extends APIError {}

/** The request never got an HTTP answer (DNS, refused, reset). */
export class APIConnectionError extends AllternitError {
  readonly cause: unknown;
  constructor(message: string, cause?: unknown) {
    super(message);
    this.cause = cause;
  }
}

/** The request took longer than `timeout`. */
export class APITimeoutError extends APIConnectionError {}

/** Build the right error class for a status and an error body. */
export function errorFor(status: number, body: ErrorBody | undefined, headers?: Headers, fallback?: string): APIError {
  const args = [status, body, headers, fallback] as const;
  if (status === 400 || status === 422) return new InvalidRequestError(...args);
  if (status === 401) return new AuthenticationError(...args);
  if (status === 402) return new PaymentRequiredError(...args);
  if (status === 403) return new PermissionError(...args);
  if (status === 404) return new NotFoundError(...args);
  if (status === 409) return new ConflictError(...args);
  if (status === 429) return new RateLimitError(...args);
  if (status >= 500) return new InternalServerError(...args);
  // Errors reported inside an already-open stream carry only a type.
  if (status === 0) {
    switch (body?.type) {
      case "invalid_request_error": return new InvalidRequestError(...args);
      case "authentication_error": return new AuthenticationError(...args);
      case "billing_error": return new PaymentRequiredError(...args);
      case "permission_error": return new PermissionError(...args);
      case "not_found_error": return new NotFoundError(...args);
      case "conflict_error": return new ConflictError(...args);
      case "rate_limit_error": return new RateLimitError(...args);
    }
  }
  return new APIError(...args);
}
